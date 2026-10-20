#!/usr/bin/env python3
"""Build the isolated audio worker and inspector from pinned codec sources."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
LOCK = json.loads((ROOT / 'scripts/audio-backend.lock.json').read_text())
OUT = ROOT / 'src-tauri/resources/audio-backend'
SPEC = importlib.util.spec_from_file_location('image_builder', ROOT / 'scripts/prepare-image-backend.py')
COMMON = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(COMMON)


def configuration(system, arch):
    pcm = 'pcm_u8,pcm_s16le,pcm_s24le,pcm_s32le,pcm_f32le,pcm_f64le'
    result = [
        '--disable-everything', '--disable-autodetect', '--disable-network',
        '--disable-doc', '--disable-debug', '--disable-shared', '--enable-static',
        '--disable-avdevice', '--enable-swscale', '--disable-ffplay', '--disable-x86asm',
        '--enable-ffmpeg', '--enable-ffprobe', '--enable-protocol=file,pipe',
        '--enable-demuxer=wav,flac,mov,matroska,avi,rawvideo,mp3,aac,ogg,pcm_f64le',
        '--enable-muxer=wav,flac,ipod,mp4,mov,matroska,rawvideo,framehash,mp3,opus,ogg,pcm_f64le,pcm_s32le,null',
        f'--enable-decoder={pcm},flac,alac,mp3float,aac,opus,vorbis,h264,hevc,vp8,vp9,mpeg4,mjpeg,prores,rawvideo,mov_text',
        f'--enable-encoder={pcm},flac,alac,libmp3lame,aac,libopus,libvorbis,libx264,rawvideo,wrapped_avframe',
        '--enable-gpl', '--enable-libx264', '--enable-libmp3lame', '--enable-libopus', '--enable-libvorbis', '--pkg-config-flags=--static',
        '--enable-parser=flac,mpegaudio,aac,opus,vorbis,h264,hevc,vp8,vp9,mpeg4video,mjpeg',
        '--enable-bsf=setts',
        '--enable-filter=aresample,aformat,anull,scale,format,fps,trim,transpose,hflip,vflip,setsar',
    ]
    if system == 'darwin':
        result += ['--enable-videotoolbox', '--enable-encoder=h264_videotoolbox',
                   '--cc=clang', f'--arch={"aarch64" if arch == "arm64" else arch}',
                   f'--extra-cflags=-arch {arch} -mmacosx-version-min={LOCK["minimumMacOS"]}',
                   f'--extra-ldflags=-arch {arch} -mmacosx-version-min={LOCK["minimumMacOS"]}']
    elif system == 'windows':
        result += ['--enable-ffnvcodec', '--enable-nvenc', '--enable-amf',
                   '--enable-libvpl', '--enable-d3d11va',
                   '--enable-encoder=h264_nvenc,h264_amf,h264_qsv',
                   '--target-os=mingw32', '--arch=x86_64', '--cc=/ucrt64/bin/gcc',
                   '--extra-ldflags=-static -static-libgcc -static-libstdc++']
    else:
        result += ['--enable-ffnvcodec', '--enable-nvenc', '--enable-vaapi', '--enable-libdrm',
                   '--enable-encoder=h264_nvenc,h264_vaapi', '--enable-filter=hwupload']
    return result


def patch_vpl_mingw_string_functions(source):
    # oneVPL 2.16 treats an undefined _MSC_VER as old MSVC and replaces
    # MinGW's secure string functions with statement macros. New Windows
    # headers use wcscpy_s in an expression, which those macros break.
    path = source / 'libvpl/src/windows/mfx_dispatcher_defs.h'
    original = '#if _MSC_VER < 1400'
    corrected = '#if defined(_MSC_VER) && _MSC_VER < 1400'
    text = path.read_text()
    if text.count(corrected) == 1 and original not in text:
        return
    if text.count(original) != 1 or corrected in text:
        raise SystemExit('Pinned oneVPL MinGW patch no longer matches; review the upstream source.')
    path.write_text(text.replace(original, corrected))


def build_hardware(work, logs, env, system, arch):
    """Build only dispatchers/headers. GPU runtimes come from the OS/driver."""
    builds = {}
    stage = work / 'install'
    for dependency in LOCK['dependencies']:
        if 'systems' not in dependency or system not in dependency['systems']:
            continue
        name = dependency['name']
        source = COMMON.extract(dependency, work / name)
        config = []
        if name == 'ffnvcodec':
            config = [f'PREFIX={stage}', 'LIBDIR=lib']
            COMMON.run(['make', 'install', *config], cwd=source, env=env, log=logs / 'ffnvcodec-install.log')
        elif name == 'amf':
            shutil.copytree(source / 'AMF', stage / 'include/AMF', dirs_exist_ok=True)
            config = ['headers-only']
        elif name == 'vpl':
            patch_vpl_mingw_string_functions(source)
            build = work / 'vpl-build'
            config = COMMON.cmake_arguments(stage, system, arch) + [
                '-DBUILD_SHARED_LIBS=OFF', '-DBUILD_TESTS=OFF', '-DBUILD_EXAMPLES=OFF',
                '-DINSTALL_EXAMPLES=OFF', '-DBUILD_EXPERIMENTAL=OFF',
                # FFmpeg links C code with gcc. oneVPL leaves its pkg-config
                # C++ runtime field empty unless the static builder supplies it.
                '-DCXX_LIB=-lstdc++ -lwinpthread',
            ]
            COMMON.run(['cmake', '-S', str(source), '-B', str(build), *config], env=env, log=logs / 'vpl-configure.log')
            COMMON.run(['cmake', '--build', str(build), '--parallel', COMMON.JOBS], env=env, log=logs / 'vpl-build.log')
            COMMON.run(['cmake', '--install', str(build)], env=env, log=logs / 'vpl-install.log')
        elif name in ['drm', 'va']:
            build = work / f'{name}-build'
            config = [f'--prefix={stage}', '--libdir=lib', '--buildtype=release',
                      '--default-library=static', '--auto-features=disabled', '-Db_staticpic=true']
            if name == 'drm':
                config += ['-Dtests=false', '-Dinstall-test-programs=false', '-Dudev=false']
            else:
                # Upstream hardcodes shared_library; honor Meson's static mode.
                path = source / 'va/meson.build'
                text = path.read_text()
                for lib in ['libva', 'libva_drm']:
                    original = f'{lib} = shared_library('
                    corrected = f'{lib} = library('
                    if original in text:
                        if text.count(original) != 1:
                            raise SystemExit('Review pinned libva static-library patch.')
                        text = text.replace(original, corrected)
                    elif corrected not in text:
                        raise SystemExit('Review pinned libva static-library patch.')
                path.write_text(text)
                triplet = 'aarch64-linux-gnu' if arch == 'arm64' else 'x86_64-linux-gnu'
                config += ['-Dwith_x11=no', '-Dwith_glx=no', '-Dwith_wayland=no',
                           '-Dwith_win32=no', '-Denable_docs=false', '-Ddisable_drm=false',
                           '--sysconfdir=/etc',
                           f'-Ddriverdir=/usr/lib/{triplet}/dri:/usr/lib64/dri:/usr/lib/dri']
            COMMON.run(['meson', 'setup', str(build), str(source), *config], env=env, log=logs / f'{name}-configure.log')
            COMMON.run(['meson', 'compile', '-C', str(build), '-j', COMMON.JOBS], env=env, log=logs / f'{name}-build.log')
            COMMON.run(['meson', 'install', '-C', str(build)], env=env, log=logs / f'{name}-install.log')
        builds[name] = (source, config)
    return builds


def build_lame(work, logs, env, system, arch):
    dependency = next(item for item in LOCK['dependencies'] if item['name'] == 'lame')
    source = COMMON.extract(dependency, work / 'lame')
    stage = work / 'install'
    flags = '-O2 -fPIC -std=gnu11'
    cc = '/ucrt64/bin/gcc' if system == 'windows' else 'clang' if system == 'darwin' else 'gcc'
    if system == 'darwin':
        flags += f' -arch {arch} -mmacosx-version-min={LOCK["minimumMacOS"]}'
    build_env = env | {'CC': cc, 'CFLAGS': flags, 'ac_cv_prog_cc_c23': 'no'}
    config = [f'--prefix={stage}', '--disable-shared', '--enable-static',
              '--disable-frontend', '--disable-decoder', '--disable-analyzer-hooks',
              '--disable-nasm', '--disable-dependency-tracking']
    if system == 'darwin':
        config += [f'--host={"aarch64" if arch == "arm64" else arch}-apple-darwin']
    COMMON.run(['sh', str(source / 'configure'), *config], cwd=source, env=build_env, log=logs / 'lame-configure.log')
    COMMON.run(['make', '-j', COMMON.JOBS], cwd=source, env=build_env, log=logs / 'lame-build.log')
    COMMON.run(['make', 'install'], cwd=source, env=build_env, log=logs / 'lame-install.log')
    return source, stage, config


def build_opus(work, logs, env, system, arch):
    dependency = next(item for item in LOCK['dependencies'] if item['name'] == 'opus')
    source = COMMON.extract(dependency, work / 'opus')
    stage = work / 'install'
    build = work / 'opus-build'
    config = COMMON.cmake_arguments(stage, system, arch) + [
        '-DBUILD_SHARED_LIBS=OFF', '-DOPUS_BUILD_SHARED_LIBRARY=OFF',
        '-DOPUS_BUILD_PROGRAMS=OFF', '-DOPUS_BUILD_TESTING=OFF',
        '-DOPUS_DRED=OFF', '-DOPUS_OSCE=OFF', '-DOPUS_DEEP_PLC=OFF',
    ]
    COMMON.run(['cmake', '-S', str(source), '-B', str(build), *config], env=env, log=logs / 'opus-configure.log')
    COMMON.run(['cmake', '--build', str(build), '--parallel', COMMON.JOBS], env=env, log=logs / 'opus-build.log')
    COMMON.run(['cmake', '--install', str(build)], env=env, log=logs / 'opus-install.log')
    return source, config


def build_x264(work, logs, env, system, arch):
    dependency = next(item for item in LOCK['dependencies'] if item['name'] == 'x264')
    source = COMMON.extract(dependency, work / 'x264')
    flags = '-O2 -fPIC'
    cc = '/ucrt64/bin/gcc' if system == 'windows' else 'clang' if system == 'darwin' else 'gcc'
    ldflags = ''
    if system == 'darwin':
        flags += f' -arch {arch} -mmacosx-version-min={LOCK["minimumMacOS"]}'
        ldflags = f'-arch {arch} -mmacosx-version-min={LOCK["minimumMacOS"]}'
    config = [f'--prefix={work / "install"}', '--enable-static', '--enable-pic',
              '--disable-cli', '--disable-opencl', '--disable-lavf', '--disable-swscale',
              '--disable-ffms', '--disable-avs']
    if system == 'darwin':
        config += [f'--host={"aarch64" if arch == "arm64" else arch}-apple-darwin']
    build_env = env | {'CC': cc, 'CFLAGS': flags, 'LDFLAGS': ldflags}
    COMMON.run(['sh', str(source / 'configure'), *config], cwd=source, env=build_env, log=logs / 'x264-configure.log')
    COMMON.run(['make', '-j', COMMON.JOBS], cwd=source, env=build_env, log=logs / 'x264-build.log')
    COMMON.run(['make', 'install-lib-static'], cwd=source, env=build_env, log=logs / 'x264-install.log')
    return source, config


def patch_mp3_padding(source):
    # A delayed encoder can emit padding across more than one final packet.
    # FFmpeg 9.0.2 overwrites the preceding packet's discard count, extending
    # some outputs (e.g. 12117 mono samples at 12 kHz) by 26 samples on decode.
    # Accumulate contiguous discard counts; ordinary packets reset padding.
    path = source / 'libavformat/mp3enc.c'
    original = 'mp3->padding = FFMAX((int64_t)AV_RL32(side_data + 4) + 528 + 1, 0);'
    corrected = 'mp3->padding = FFMAX((int64_t)AV_RL32(side_data + 4) + (mp3->padding ? mp3->padding : 528 + 1), 0);'
    text = path.read_text()
    if corrected in text:
        return
    if text.count(original) != 1:
        raise SystemExit('Pinned MP3 padding patch no longer matches; review the upstream source.')
    path.write_text(text.replace(original, corrected))


def patch_vorbis_single_page_timing(source):
    # The first-packet path excludes EOS pages. When all audio fits on one
    # page, the demuxer counts the initial overlap as output and over-trims
    # the end (or emits padding for very short clips). Set the first packet's
    # negative overlap timestamp before the ordinary EOS accounting. Seeking
    # back to the start resets lastpts to zero, so this also handles re-reads.
    path = source / 'libavformat/oggparsevorbis.c'
    original = '''    if (os->flags & OGG_FLAG_EOS) {
        if (os->lastpts != AV_NOPTS_VALUE) {'''
    corrected = '''    if (os->flags & OGG_FLAG_EOS) {
        if (os->lastpts == 0)
            os->lastpts = os->lastdts = -(int64_t)os->pduration;
        if (os->lastpts != AV_NOPTS_VALUE) {'''
    text = path.read_text()
    if corrected in text:
        return
    if text.count(original) != 1:
        raise SystemExit('Pinned Vorbis timing patch no longer matches; review the upstream source.')
    path.write_text(text.replace(original, corrected))


def patch_mp4_audio_priming(source):
    # A positive audio timestamp makes movenc emit an empty edit followed by
    # untrimmed media. AAC re-encoding then exposes its new encoder priming.
    # Account for that priming in both the presentation delay and media start;
    # zero-start audio continues through the existing negative-DTS trim path.
    path = source / 'libavformat/movenc.c'
    original = '''    delay = av_rescale_rnd(start_dts + start_ct, mov->movie_timescale,
                           track->timescale, AV_ROUND_DOWN);'''
    corrected = '''    /* Recast: retain delayed AAC presentation while trimming encoder priming. */
    if (track->par->codec_id == AV_CODEC_ID_AAC && track->par->initial_padding > 0) {
        int64_t priming = av_rescale_q(track->par->initial_padding,
                                      (AVRational){1, track->par->sample_rate},
                                      (AVRational){1, track->timescale});
        if (start_dts + start_ct + priming > 0) {
            start_ct += priming;
            duration -= av_rescale_rnd(priming, mov->movie_timescale,
                                      track->timescale, AV_ROUND_DOWN);
        }
    }
    delay = av_rescale_rnd(start_dts + start_ct, mov->movie_timescale,
                           track->timescale, AV_ROUND_DOWN);'''
    text = path.read_text()
    if corrected not in text:
        if text.count(original) != 1:
            raise SystemExit('Pinned MP4 audio priming patch no longer matches; review the upstream source.')
        path.write_text(text.replace(original, corrected))

    # The demuxer's trailing-padding check also needs the presentation endpoint,
    # not a zero-based duration. Otherwise delayed AAC exposes its final padding.
    path = source / 'libavformat/mov.c'
    patches = [
        (
            '    st->duration = FFMIN(st->duration, edit_list_dts_entry_end - start_dts);',
            '''    /* Recast: empty audio edits are start offsets, not playable duration. */
    st->duration = FFMIN(st->duration, edit_list_dts_entry_end - start_dts -
                        (st->codecpar->codec_type == AVMEDIA_TYPE_AUDIO ? empty_edits_sum_duration : 0));''',
        ),
        (
            '''        int64_t total = av_rescale_q(st->duration, st->time_base, (AVRational){ 1, st->codecpar->sample_rate });
        int64_t duration = pkt->duration;

        if (st->duration < pkt->pts) {
            duration = 0;
        } else
            duration = FFMIN(duration, (uint64_t)st->duration - pkt->pts);''',
            '''        /* Recast: clip AAC padding against the end of its presentation interval. */
        int64_t end = av_sat_add64(st->duration, st->start_time == AV_NOPTS_VALUE ? 0 : st->start_time);
        int64_t total = av_rescale_q(end, st->time_base, (AVRational){ 1, st->codecpar->sample_rate });
        int64_t duration = pkt->duration;

        if (end < pkt->pts) {
            duration = 0;
        } else
            duration = FFMIN(duration, (uint64_t)end - pkt->pts);''',
        ),
    ]
    text = path.read_text()
    for original, corrected in patches:
        if corrected in text:
            continue
        if text.count(original) != 1:
            raise SystemExit('Pinned MP4 audio endpoint patch no longer matches; review the upstream source.')
        text = text.replace(original, corrected)
    path.write_text(text)



def patch_aac_discard_padding(source):
    # Like libopusenc, publish exact trailing samples before muxer time-base
    # rounding. Matroska needs DiscardPadding; MP4 already uses edit durations.
    path = source / 'libavcodec/aacenc.c'
    original = """    ff_af_queue_remove(&s->afq, avctx->frame_size, &avpkt->pts,
                       &avpkt->duration);

    avpkt->flags |= AV_PKT_FLAG_KEY;"""
    corrected = """    ff_af_queue_remove(&s->afq, avctx->frame_size, &avpkt->pts,
                       &avpkt->duration);

    /* Recast: retain exact final AAC length in containers with DiscardPadding. */
    int discard_padding = avctx->frame_size - ff_samples_from_time_base(avctx, avpkt->duration);
    if (discard_padding > 0) {
        uint8_t *side_data = av_packet_new_side_data(avpkt, AV_PKT_DATA_SKIP_SAMPLES, 10);
        if (!side_data)
            return AVERROR(ENOMEM);
        AV_WL32(side_data + 4, discard_padding);
    }

    avpkt->flags |= AV_PKT_FLAG_KEY;"""
    text = path.read_text()
    if corrected in text:
        return
    if text.count(original) != 1:
        raise SystemExit('Pinned AAC discard-padding patch no longer matches; review the upstream source.')
    path.write_text(text.replace(original, corrected))



def patch_matroska_playable_duration(source):
    # CodecDelay shifts block timestamps, not the end of the presentation.
    # DiscardPadding can trim a full packet or an already shortened duration.
    path = source / 'libavformat/matroskaenc.c'
    original = """    mkv->duration   = FFMAX(mkv->duration,   ts + duration);
    track->duration = FFMAX(track->duration, ts + duration);"""
    corrected = """    /* Recast: report playable duration, excluding codec delay and padding. */
    int64_t presentation_duration = duration;
    if (par->codec_type == AVMEDIA_TYPE_AUDIO) {
        size_t padding_size;
        const uint8_t *padding = av_packet_get_side_data(pkt, AV_PKT_DATA_SKIP_SAMPLES, &padding_size);
        int frame_samples = av_get_audio_frame_duration2(par, pkt->size);
        if (padding && padding_size >= 10 && frame_samples > 0) {
            int64_t valid_samples = FFMAX(0, (int64_t)frame_samples - AV_RL32(padding + 4));
            int64_t valid_duration = av_rescale_q(valid_samples,
                (AVRational){1, par->sample_rate}, st->time_base);
            presentation_duration = FFMIN(duration, valid_duration);
        }
    }
    int64_t presentation_end = ts - track->ts_offset + presentation_duration;
    mkv->duration   = FFMAX(mkv->duration,   presentation_end);
    track->duration = FFMAX(track->duration, presentation_end);"""
    text = path.read_text()
    if corrected in text:
        return
    if text.count(original) != 1:
        raise SystemExit('Pinned Matroska duration patch no longer matches; review the upstream source.')
    path.write_text(text.replace(original, corrected))


def build_vorbis(work, logs, env, system, arch):
    builds = {}
    stage = work / 'install'
    for name in ['ogg', 'vorbis']:
        dependency = next(item for item in LOCK['dependencies'] if item['name'] == name)
        source = COMMON.extract(dependency, work / name)
        build = work / f'{name}-build'
        config = COMMON.cmake_arguments(stage, system, arch) + [
            '-DBUILD_SHARED_LIBS=OFF', '-DBUILD_TESTING=OFF',
            '-DCMAKE_POLICY_VERSION_MINIMUM=3.5',
        ]
        if name == 'ogg':
            config += ['-DINSTALL_DOCS=OFF']
        else:
            # Do not discover a Homebrew/MSYS shared Ogg library or headers.
            config += [f'-DOGG_LIBRARY={stage / "lib/libogg.a"}',
                       f'-DOGG_INCLUDE_DIR={stage / "include"}']
            if system == 'linux':
                # Vorbis 1.3.7's CMake build leaves VORBIS_LIBS unset, so its
                # pkg-config file omits libm and FFmpeg's static link probe fails.
                config += ['-DVORBIS_LIBS=-lm']
        COMMON.run(['cmake', '-S', str(source), '-B', str(build), *config], env=env, log=logs / f'{name}-configure.log')
        COMMON.run(['cmake', '--build', str(build), '--parallel', COMMON.JOBS], env=env, log=logs / f'{name}-build.log')
        COMMON.run(['cmake', '--install', str(build)], env=env, log=logs / f'{name}-install.log')
        builds[name] = (source, config)
    return builds


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--arch', choices=['arm64', 'x86_64'])
    args = parser.parse_args()
    host = {'aarch64': 'arm64', 'amd64': 'x86_64'}.get(platform.machine().lower(), platform.machine().lower())
    system = platform.system().lower()
    if os.environ.get('MSYSTEM'):
        if os.environ['MSYSTEM'] != 'UCRT64' or os.name == 'nt':
            raise SystemExit('Run /usr/bin/python in MSYS2 UCRT64 for the Windows worker.')
        system, host = 'windows', 'x86_64'
    arch = args.arch or host
    if system not in ('darwin', 'linux', 'windows') or arch not in ('arm64', 'x86_64'):
        raise SystemExit(f'Unsupported build host: {system}/{arch}')
    if system != 'darwin' and arch != host:
        raise SystemExit('Use a native Linux/Windows build host.')
    recipe = Path(__file__).read_bytes() + (ROOT / 'scripts/prepare-image-backend.py').read_bytes()
    fingerprint = hashlib.sha256(json.dumps(LOCK, sort_keys=True).encode() + recipe).hexdigest()[:12]
    work = ROOT / '.backend-build' / f'audio-{system}-{arch}-{fingerprint}'
    logs = work / 'logs'
    logs.mkdir(parents=True, exist_ok=True)
    source = COMMON.extract(LOCK['source'], work / 'source')
    patch_mp3_padding(source)
    patch_vorbis_single_page_timing(source)
    patch_mp4_audio_priming(source)
    patch_aac_discard_padding(source)
    patch_matroska_playable_duration(source)
    env = os.environ.copy()
    for key in ['CPATH', 'C_INCLUDE_PATH', 'CPLUS_INCLUDE_PATH', 'LIBRARY_PATH', 'SDKROOT',
                'CFLAGS', 'CXXFLAGS', 'CPPFLAGS', 'LDFLAGS', 'LIBS', 'CC', 'CXX',
                'PKG_CONFIG_PATH', 'PKG_CONFIG', 'mpg123_CFLAGS', 'mpg123_LIBS']:
        env.pop(key, None)
    env['PKG_CONFIG_LIBDIR'] = str(work / 'no-system-packages')
    if system == 'darwin':
        env['MACOSX_DEPLOYMENT_TARGET'] = LOCK['minimumMacOS']
    lame_source, stage, lame_config = build_lame(work, logs, env, system, arch)
    opus_source, opus_config = build_opus(work, logs, env, system, arch)
    dependency_builds = {'lame': (lame_source, lame_config), 'opus': (opus_source, opus_config)}
    # Expose only our pinned static libraries to FFmpeg's pkg-config probes.
    env['PKG_CONFIG_LIBDIR'] = str(stage / 'lib/pkgconfig')
    dependency_builds.update(build_vorbis(work, logs, env, system, arch))
    dependency_builds['x264'] = build_x264(work, logs, env, system, arch)
    dependency_builds.update(build_hardware(work, logs, env, system, arch))
    config = configuration(system, arch)
    config += [f'--extra-cflags=-I{shlex.quote(str(stage / "include"))}',
               f'--extra-ldflags=-L{shlex.quote(str(stage / "lib"))}']
    if arch != host:
        config += ['--enable-cross-compile', '--target-os=darwin']
    print(f'Building FFmpeg {LOCK["source"]["version"]} for {system}/{arch}…', flush=True)
    try:
        COMMON.run(['sh', str(source / 'configure'), *config], cwd=source, env=env, log=logs / 'configure.log')
    finally:
        # Keep the compiler/linker diagnostics in CI's uploaded log inventory,
        # including when configure reports only a generic dependency failure.
        diagnostic = source / 'ffbuild/config.log'
        if diagnostic.exists():
            shutil.copyfile(diagnostic, logs / 'ffmpeg-config.log')
    required = {'darwin': ['videotoolbox'], 'windows': ['nvenc', 'qsv', 'amf'],
                'linux': ['nvenc', 'vaapi']}[system]
    components = (source / 'config_components.h').read_text()
    for encoder in required:
        if f'#define CONFIG_H264_{encoder.upper()}_ENCODER 1' not in components:
            raise SystemExit(f'Required hardware encoder h264_{encoder} was not compiled in.')
    # FFmpeg's Make targets include EXESUF, which is .exe on Windows.
    programs = {name: name + ('.exe' if system == 'windows' else '') for name in ['ffmpeg', 'ffprobe']}
    COMMON.run(['make', '-j', COMMON.JOBS, *programs.values()], cwd=source, env=env, log=logs / 'build.log')
    OUT.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.audio-backend-', dir=OUT.parent) as temporary:
        output = Path(temporary)
        binaries = {}
        for name, filename in programs.items():
            binary = output / filename
            shutil.copy2(source / filename, binary)
            COMMON.run(['strip', '-x' if system == 'darwin' else '--strip-unneeded', str(binary)])
            if system == 'darwin':
                COMMON.run(['codesign', '--force', '--sign', '-', str(binary)])
            audited = COMMON.audit(binary, system, arch)
            if arch == host:
                minimal_env = env.copy()
                minimal_env['PATH'] = '/c/Windows/System32' if system == 'windows' else '/usr/bin:/bin'
                version = subprocess.check_output([str(binary), '-version'], env=minimal_env, text=True)
                if f'{name} version {LOCK["source"]["version"]}' not in version:
                    raise SystemExit(f'Unexpected worker version: {version}')
            binaries[filename] = {'sha256': COMMON.sha(binary), **audited}
        notices = output / 'licenses'
        notices.mkdir()
        for name in ['COPYING.LGPLv2.1', 'COPYING.GPLv2', 'LICENSE.md']:
            shutil.copyfile(source / name, notices / name)
        dependency_notices = ''
        dependencies = [d for d in LOCK['dependencies'] if system in d.get('systems', [system])]
        for dependency in dependencies:
            name = dependency['name']
            dependency_source, dependency_config = dependency_builds[name]
            for license_name in dependency['licenses']:
                shutil.copyfile(dependency_source / license_name, notices / f'{name}-{Path(license_name).name}')
            dependency_notices += (f'{name} source: {dependency["url"]}\n'
                                   f'{name} SHA-256: {dependency["sha256"]}\n'
                                   f'{name} configuration: {shlex.join(dependency_config)}\n')
        (output / 'SOURCE.txt').write_text(
            'FFmpeg source: ' + LOCK['source']['url'] + '\n'
            'Source SHA-256: ' + LOCK['source']['sha256'] + '\n'
            'FFmpeg MP3 trailing-padding accumulation corrected by a checked source patch.\n'
            'FFmpeg single-audio-page Ogg Vorbis overlap timestamp corrected by a checked source patch.\n'
            'On Linux, libva Meson targets honor static-library mode via a checked build-file patch.\n'
            'Rebuild with scripts/prepare-audio-backend.py.\n'
            'Configuration: ' + ' '.join(config) + '\n'
            + dependency_notices)
        manifest = {'system': system, 'architecture': arch, 'source': LOCK['source'],
                    'dependencies': dependencies,
                    'recipeFingerprint': fingerprint, 'configuration': config, 'binaries': binaries}
        (output / 'build-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        backup = OUT.with_name('.audio-backend-previous')
        if backup.exists():
            raise SystemExit(f'Review the previous backend backup: {backup}')
        if OUT.exists():
            OUT.rename(backup)
        try:
            output.rename(OUT)
        except BaseException:
            if backup.exists():
                backup.rename(OUT)
            raise
        if backup.exists():
            shutil.rmtree(backup)
    print(f'Bundled audio worker: {OUT}', flush=True)


if __name__ == '__main__':
    main()
