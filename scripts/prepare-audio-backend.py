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
        '--disable-avdevice', '--disable-swscale', '--disable-ffplay', '--disable-x86asm',
        '--enable-ffmpeg', '--enable-ffprobe', '--enable-protocol=file,pipe',
        '--enable-demuxer=wav,flac,mov,mp3,aac,ogg,pcm_f64le',
        '--enable-muxer=wav,flac,ipod,mp3,opus,pcm_f64le,pcm_s32le,null',
        f'--enable-decoder={pcm},flac,alac,mp3float,aac,opus', f'--enable-encoder={pcm},flac,alac,libmp3lame,aac,libopus',
        '--enable-libmp3lame', '--enable-libopus', '--pkg-config-flags=--static',
        '--enable-parser=flac,mpegaudio,aac,opus',
        '--enable-filter=aresample,aformat,anull',
    ]
    if system == 'darwin':
        result += ['--cc=clang', f'--arch={"aarch64" if arch == "arm64" else arch}',
                   f'--extra-cflags=-arch {arch} -mmacosx-version-min={LOCK["minimumMacOS"]}',
                   f'--extra-ldflags=-arch {arch} -mmacosx-version-min={LOCK["minimumMacOS"]}']
    elif system == 'windows':
        result += ['--target-os=mingw32', '--arch=x86_64', '--cc=/ucrt64/bin/gcc',
                   '--extra-ldflags=-static -static-libgcc']
    return result


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
    config = configuration(system, arch)
    config += [f'--extra-cflags=-I{shlex.quote(str(stage / "include"))}',
               f'--extra-ldflags=-L{shlex.quote(str(stage / "lib"))}']
    if arch != host:
        config += ['--enable-cross-compile', '--target-os=darwin']
    print(f'Building FFmpeg {LOCK["source"]["version"]} for {system}/{arch}…', flush=True)
    COMMON.run(['sh', str(source / 'configure'), *config], cwd=source, env=env, log=logs / 'configure.log')
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
        for name in ['COPYING.LGPLv2.1', 'LICENSE.md']:
            shutil.copyfile(source / name, notices / name)
        dependency_notices = ''
        for dependency in LOCK['dependencies']:
            name = dependency['name']
            dependency_source, dependency_config = dependency_builds[name]
            for license_name in dependency['licenses']:
                shutil.copyfile(dependency_source / license_name, notices / f'{name}-{license_name}')
            dependency_notices += (f'{name} source (unmodified): {dependency["url"]}\n'
                                   f'{name} SHA-256: {dependency["sha256"]}\n'
                                   f'{name} configuration: {shlex.join(dependency_config)}\n')
        (output / 'SOURCE.txt').write_text(
            'FFmpeg source: ' + LOCK['source']['url'] + '\n'
            'Source SHA-256: ' + LOCK['source']['sha256'] + '\n'
            'FFmpeg MP3 trailing-padding accumulation corrected by a checked source patch.\n'
            'Rebuild with scripts/prepare-audio-backend.py.\n'
            'Configuration: ' + ' '.join(config) + '\n'
            + dependency_notices)
        manifest = {'system': system, 'architecture': arch, 'source': LOCK['source'],
                    'dependencies': LOCK['dependencies'],
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
