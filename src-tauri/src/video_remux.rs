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
            .suffix(".mkv")
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
                "-default_mode",
                "passthrough",
                "-f",
                "matroska",
                "-progress",
            ])
            .arg(&status);
        if !job.options.metadata {
            command.args(["-map_metadata:s", "-1"]);
            for (i, stream) in streams(&job.info.probe).enumerate() {
                if let Some(language) = stream["tags"]["language"].as_str() {
                    command.args([format!("-metadata:s:{i}"), format!("language={language}")]);
                }
            }
        }
        // A MOV edit can end within its last displayed frame. Carry that visible
        // endpoint into Matroska's packet duration without touching encoded data.
        command.args([
            "-bsf:v",
            &format!(
                "setts=pts=PTS:dts=DTS:duration=min(DURATION\\,max(1\\,round({:.9}/TB)-PTS))",
                job.video_end
            ),
        ]);
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
            .ok_or_else(|| "The MKV has no video track.".to_string())?;
        if actual.codec != job.info.codec
            || actual.width != job.info.width
            || actual.height != job.info.height
            || actual.rotation != job.info.rotation
            || streams(&actual.probe).count() != streams(&job.info.probe).count()
        {
            return Err("The MKV did not retain the source tracks or geometry."
                .to_string()
                .into());
        }
        if video_end(&actual).is_none_or(|end| (end - job.video_end).abs() > 0.00101) {
            return Err("The MKV did not retain the video's visible duration."
                .to_string()
                .into());
        }
        self.check_retained_tracks(job, &actual, temporary.path(), cancel)?;
        progress(0.85, "Checking MKV");
        let checked =
            self.video_fingerprint(temporary.path(), job.work, "output", cancel, &mut |time| {
                progress(
                    0.85 + (time / job.info.duration).clamp(0.0, 1.0) * 0.14,
                    "Checking MKV",
                )
            })?;
        validate_timing(
            job,
            &checked,
            &frame_stats(&checked, cancel)?,
            0.00101,
            cancel,
        )?;
        // Stream copy must retain decoded content as well as timestamps.
        for index in job.source_stats.keys() {
            let rows = |path: &Path| -> Result<_, ConversionError> {
                let prefix = format!("{index},");
                Ok(frame_rows(path)?.filter_map(move |line| match line {
                    Err(error) => Some(Err(ConversionError::from(error.to_string()))),
                    Ok(line) if line.starts_with(&prefix) => {
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
                        "The MKV did not retain the original decoded content. No output was saved."
                            .to_string()
                            .into(),
                    );
                }
            }
        }
        Ok(temporary)
    }

    pub(super) fn check_retained_tracks(
        &self,
        job: &VideoJob<'_>,
        actual: &VideoInfo,
        output: &Path,
        cancel: &AtomicBool,
    ) -> Result<(), ConversionError> {
        for kind in ["video", "audio", "subtitle"] {
            for (before, after) in streams(&job.info.probe)
                .filter(|s| s["codec_type"] == kind)
                .zip(streams(&actual.probe).filter(|s| s["codec_type"] == kind))
            {
                if before["codec_type"] != after["codec_type"]
                    || (job.options.copy_streams && before["codec_name"] != after["codec_name"])
                    || (job.options.target == "mkv"
                        && job.size.is_none()
                        && before["codec_type"] == "audio"
                        && before["codec_name"] != after["codec_name"])
                    || before["tags"]["language"].as_str().unwrap_or("und")
                        != after["tags"]["language"].as_str().unwrap_or("und")
                    || before["disposition"] != after["disposition"]
                    || (before["codec_type"] == "audio"
                        && (before["sample_rate"] != after["sample_rate"]
                            || before["channels"] != after["channels"]
                            || before["channel_layout"] != after["channel_layout"]))
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
