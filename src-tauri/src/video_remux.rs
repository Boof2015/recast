//! Container copying shares source validation, cancellation, and safe publication.
use super::*;

impl AudioBackend {
    pub(super) fn remux_video(
        &self,
        job: &VideoJob<'_>,
        cancel: &AtomicBool,
        progress: &mut impl FnMut(f64, &str),
    ) -> Result<tempfile::NamedTempFile, ConversionError> {
        let temporary = tempfile::Builder::new()
            .prefix(".recast-")
            .suffix(&format!(".{}", job.options.target))
            .tempfile_in(job.parent)
            .map_err(|_| "The output folder is not writable.".to_string())?;
        let status = job.work.join("copy.progress");
        progress(0.2, "Copying streams");
        let mut command = self.video_command();
        command
            .args(["-copyts", "-start_at_zero", "-i"])
            .arg(job.staged)
            .args([
                "-map",
                "0",
                "-c",
                "copy",
                "-map_metadata",
                if job.options.metadata { "0" } else { "-1" },
                "-map_chapters",
                "-1",
                "-avoid_negative_ts",
                "disabled",
                "-progress",
            ])
            .arg(&status);
        job.options.configure_muxer(&mut command, job.info)?;
        if !job.options.metadata {
            command.args(["-map_metadata:s", "-1"]);
            for (i, stream) in streams(&job.info.probe).enumerate() {
                if let Some(language) = stream["tags"]["language"].as_str() {
                    command.args([format!("-metadata:s:{i}"), format!("language={language}")]);
                }
            }
        }
        // A MOV edit can end within its last displayed frame. Carry that visible
        // endpoint into the output packet duration without touching encoded data.
        command.args([
            "-bsf:v",
            &format!(
                "setts=pts=PTS:dts=DTS:duration=min(DURATION\\,max(1\\,round({:.9}/TB)-PTS))",
                job.video_end
            ),
        ]);
        self.configure_copied_audio_timing(job, &mut command)?;
        command.arg(temporary.path());
        run(
            &mut command,
            &job.work.join("copy.log"),
            Some(&status),
            cancel,
            86400,
            &mut |time| {
                progress(
                    0.2 + (time / job.info.duration).clamp(0.0, 1.0) * 0.65,
                    "Copying streams",
                )
            },
        )?;
        let actual = self
            .inspect_video(temporary.path(), cancel)?
            .ok_or_else(|| "The output has no video track.".to_string())?;
        if actual.codec != job.info.codec
            || actual.width != job.info.width
            || actual.height != job.info.height
            || actual.rotation != job.info.rotation
            || streams(&actual.probe).count() != streams(&job.info.probe).count()
        {
            return Err("The output did not retain the source tracks or geometry."
                .to_string()
                .into());
        }
        if video_end(&actual)
            .is_none_or(|end| (end - job.video_end).abs() > job.options.precision())
        {
            return Err("The output did not retain the video's visible duration."
                .to_string()
                .into());
        }
        self.check_retained_tracks(job, &actual, temporary.path(), cancel)?;
        let checking = format!("Checking {}", job.options.target.to_uppercase());
        progress(0.85, &checking);
        let checked = self.video_fingerprint(
            temporary.path(),
            job.work,
            "output",
            job.info.audio_tracks,
            cancel,
            &mut |time| {
                progress(
                    0.85 + (time / job.info.duration).clamp(0.0, 1.0) * 0.14,
                    &checking,
                )
            },
        )?;
        validate_timing(
            job,
            &checked,
            &frame_stats(&checked, cancel)?,
            job.options.precision(),
            cancel,
        )?;
        // Stream copy must retain decoded content as well as timestamps.
        let rows = |path: &Path| -> Result<_, ConversionError> {
            Ok(frame_rows(path)?.filter_map(move |line| match line {
                Err(error) => Some(Err(ConversionError::from(error.to_string()))),
                Ok(line) if line.starts_with("0,") => {
                    let parts: Vec<_> = line.split(',').map(str::trim).collect();
                    Some(Ok(format!("{}:{}", parts[4], parts[5])))
                }
                _ => None,
            }))
        };
        let mut before = rows(job.original)?;
        let mut after = rows(&checked)?;
        loop {
            check_cancel(cancel)?;
            let matches = match (before.next(), after.next()) {
                (None, None) => break,
                (Some(a), Some(b)) => a? == b?,
                _ => false,
            };
            if !matches {
                return Err(
                    "The output did not retain the original decoded content. No output was saved."
                        .to_string()
                        .into(),
                );
            }
        }
        self.check_copied_audio(job)?;
        Ok(temporary)
    }

    pub(super) fn configure_copied_audio_timing(
        &self,
        job: &VideoJob<'_>,
        command: &mut Command,
    ) -> Result<(), ConversionError> {
        if !["mp4", "mov"].contains(&job.options.target.as_str()) {
            return Ok(());
        }
        let target = job.options.target.to_uppercase();
        for (i, audio) in streams(&job.info.probe)
            .filter(|s| s["codec_type"] == "audio")
            .enumerate()
        {
            let pcm = audio["codec_name"]
                .as_str()
                .unwrap_or("")
                .starts_with("pcm_");
            let matroska_framed = ["aac", "alac"]
                .contains(&audio["codec_name"].as_str().unwrap_or(""))
                && job.info.container == "matroska,webm";
            if !job.options.copies_audio(audio) || (!pcm && !matroska_framed) {
                continue;
            }
            let stats = &job.source_stats[&(i + 1)];
            if stats.discontinuity > 0.00101 {
                return Err(format!("This audio track has timing gaps that {target} cannot retain. Choose MKV to keep its timing.").into());
            }
            let rate = num(&audio["sample_rate"]).unwrap() as u32;
            let end = stats.start + stats.samples as f64 / f64::from(rate);
            let filter = if matroska_framed {
                // Reconstruct the exact AAC/ALAC sample clock from its frame length,
                // decoded start/count, and priming. Millisecond packet times are
                // insufficient to express a sample-accurate MP4/MOV edit list.
                let frame = if audio["codec_name"] == "aac" { aac_frame_samples(audio) } else { alac_frame_samples(audio) }.ok_or_else(|| format!("This audio configuration cannot be copied to {target} yet. Choose Target size to re-encode it, or MKV to retain it."))?;
                let priming = num(&audio["initial_padding"]).unwrap_or(0.0) as u32;
                let start = (stats.start * f64::from(rate)).round() as i64 - i64::from(priming);
                format!("setts=time_base=1/{rate}:prescale=1:ts={start}+N*{frame}:duration=if(eq(NEXT_PTS\\,NOPTS)\\,round({end:.9}*SR)-({start}+N*{frame})\\,{frame})")
            } else {
                // Repacketize unchanged PCM bytes without padding, then use the
                // exact sample clock. Rounded input packet timestamps otherwise
                // create MOV edit lists that clip samples, especially with delay.
                let start = (stats.start * f64::from(rate)).round() as i64;
                format!("pcm_rechunk=n=1024:p=0,setts=ts={start}+N*1024")
            };
            command.args([format!("-bsf:a:{i}"), filter]);
        }
        Ok(())
    }

    pub(super) fn check_copied_audio(&self, job: &VideoJob<'_>) -> Result<(), ConversionError> {
        if job.info.audio_tracks == 0 {
            return Ok(());
        }
        let before = std::fs::read_to_string(job.work.join("source.audio-hashes"))
            .map_err(|e| e.to_string())?;
        let after = std::fs::read_to_string(job.work.join("output.audio-hashes"))
            .map_err(|e| e.to_string())?;
        let before: Vec<_> = before.lines().collect();
        let after: Vec<_> = after.lines().collect();
        if before.len() != job.info.audio_tracks
            || after.len() != before.len()
            || streams(&job.info.probe)
                .filter(|s| s["codec_type"] == "audio")
                .enumerate()
                .any(|(i, s)| job.options.copies_audio(s) && before[i] != after[i])
        {
            return Err(
                "The output did not retain the original audio samples. No output was saved."
                    .to_string()
                    .into(),
            );
        }
        Ok(())
    }

    pub(super) fn check_retained_tracks(
        &self,
        job: &VideoJob<'_>,
        actual: &VideoInfo,
        output: &Path,
        cancel: &AtomicBool,
    ) -> Result<(), ConversionError> {
        for kind in ["video", "audio", "subtitle"] {
            for (index, (before, after)) in streams(&job.info.probe)
                .filter(|s| s["codec_type"] == kind)
                .zip(streams(&actual.probe).filter(|s| s["codec_type"] == kind))
                .enumerate()
            {
                let mut disposition = before["disposition"].clone();
                if job.options.target == "mov"
                    && index == 0
                    && !streams(&job.info.probe)
                        .any(|s| s["codec_type"] == kind && s["disposition"]["default"] == 1)
                {
                    // MOV enables the first track of each kind when none is selected.
                    disposition["default"] = 1.into();
                }
                if before["codec_type"] != after["codec_type"]
                    || (job.options.copy_streams && before["codec_name"] != after["codec_name"])
                    || (job.options.copies_audio(before)
                        && before["codec_type"] == "audio"
                        && before["codec_name"] != after["codec_name"])
                    || before["tags"]["language"].as_str().unwrap_or("und")
                        != after["tags"]["language"].as_str().unwrap_or("und")
                    || disposition != after["disposition"]
                    || (before["codec_type"] == "audio"
                        && (f64::from(job.options.audio_rate(before))
                            != num(&after["sample_rate"]).unwrap_or(0.0)
                            || before["channels"] != after["channels"]
                            || audio_layout(before) != audio_layout(after)))
                {
                    return Err("The output did not retain the expected track properties."
                        .to_string()
                        .into());
                }
            }
        }
        if job.info.subtitle_tracks == 0 {
            return Ok(());
        }
        let packets = |path: &Path, name: &str| -> Result<Value, ConversionError> {
            let report = job.work.join(format!("{name}-subtitles.json"));
            let mut command = self.command("ffprobe");
            command
                .args([
                    "-v",
                    "error",
                    "-protocol_whitelist",
                    "file,pipe",
                    "-select_streams",
                    "s",
                    "-show_packets",
                    "-show_data_hash",
                    "sha256",
                    "-show_entries",
                    "packet=stream_index,pts_time,duration_time,data_hash",
                    "-of",
                    "json",
                ])
                .arg(path)
                .stdout(File::create(&report).map_err(|e| e.to_string())?);
            run(
                &mut command,
                &job.work.join(format!("{name}-subtitles.log")),
                None,
                cancel,
                300,
                &mut |_| {},
            )?;
            let mut data = Vec::new();
            File::open(&report)
                .map_err(|e| e.to_string())?
                .take(67_108_865)
                .read_to_end(&mut data)
                .map_err(|e| e.to_string())?;
            if data.len() > 67_108_864 {
                return Err("These subtitles exceed the verification budget."
                    .to_string()
                    .into());
            }
            serde_json::from_slice(&data).map_err(|_| {
                "Subtitle verification returned invalid data."
                    .to_string()
                    .into()
            })
        };
        let source = packets(job.staged, "source")?;
        let result = packets(output, "output")?;
        let before = source["packets"]
            .as_array()
            .ok_or_else(|| "Missing source subtitle packets.".to_string())?;
        let after = result["packets"]
            .as_array()
            .ok_or_else(|| "Missing output subtitle packets.".to_string())?;
        let indices = |probe: &Value| {
            streams(probe)
                .filter(|s| s["codec_type"] == "subtitle")
                .map(|s| s["index"].clone())
                .collect::<Vec<_>>()
        };
        let before_indices = indices(&job.info.probe);
        let after_indices = indices(&actual.probe);
        let before_start = num(&job.info.probe["format"]["start_time"]).unwrap_or(0.0);
        let after_start = num(&actual.probe["format"]["start_time"]).unwrap_or(0.0);
        if before.len() != after.len()
            || before.iter().zip(after).any(|(a, b)| {
                a["data_hash"] != b["data_hash"]
                    || before_indices.iter().position(|i| i == &a["stream_index"])
                        != after_indices.iter().position(|i| i == &b["stream_index"])
                    || ["pts_time", "duration_time"].iter().any(|key| {
                        match (num(&a[key]), num(&b[key])) {
                            (Some(a), Some(b)) => {
                                let shift = if *key == "pts_time" {
                                    before_start - after_start
                                } else {
                                    0.0
                                };
                                (a - b - shift).abs() > 0.00101
                            }
                            (None, None) => false,
                            _ => true,
                        }
                    })
            })
        {
            return Err(
                "The output did not retain complete subtitle content and timing."
                    .to_string()
                    .into(),
            );
        }
        Ok(())
    }
}

// AAC-LC GASpecificConfig's frameLengthFlag selects 1024 or 960 samples.
// Read only the bounded stream extradata emitted by ffprobe, not media packets.
fn codec_data(audio: &Value) -> Option<Vec<u8>> {
    let hex: String = audio["extradata"]
        .as_str()?
        .lines()
        .filter_map(|line| {
            line.split_once(": ")
                .map(|(_, bytes)| bytes.split("  ").next().unwrap_or(""))
        })
        .flat_map(|s| s.chars().filter(|c| !c.is_whitespace()))
        .collect();
    if !hex.is_ascii() || hex.len() > 8192 || !hex.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
        .collect::<Result<_, _>>()
        .ok()?;
    Some(bytes)
}

fn alac_frame_samples(audio: &Value) -> Option<u32> {
    let bytes = codec_data(audio)?;
    if bytes.len() != 36 || &bytes[4..8] != b"alac" {
        return None;
    }
    let samples = u32::from_be_bytes(bytes[12..16].try_into().ok()?);
    (samples > 0 && samples <= 65536).then_some(samples)
}

fn aac_frame_samples(audio: &Value) -> Option<u32> {
    let bytes = codec_data(audio)?;
    let mut bit = 0;
    let mut read = |count: usize| -> Option<u32> {
        let mut value = 0;
        for _ in 0..count {
            value = (value << 1) | u32::from((bytes.get(bit / 8)? >> (7 - bit % 8)) & 1);
            bit += 1;
        }
        Some(value)
    };
    if read(5)? != 2 {
        return None;
    }
    if read(4)? == 15 {
        read(24)?;
    }
    read(4)?;
    Some(if read(1)? == 0 { 1024 } else { 960 })
}
