#!/usr/bin/env python3
"""Build Recast's isolated image worker from pinned sources (macOS/Linux/MSYS2)."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shlex
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
LOCK = json.loads((ROOT / 'scripts/image-backend.lock.json').read_text())
CACHE = ROOT / '.backend-build'
OUT = ROOT / 'src-tauri/resources/image-backend'
JOBS = str(min(os.cpu_count() or 2, 8))


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def run(args, *, log=None, **kwargs):
    if log:
        with log.open('a') as output:
            output.write('\n' + shlex.join(str(a) for a in args) + '\n')
            output.flush()
            try:
                return subprocess.run(args, check=True, stdout=output, stderr=subprocess.STDOUT, **kwargs)
            except subprocess.CalledProcessError:
                print('\n'.join(log.read_text(errors='replace').splitlines()[-35:]), flush=True)
                raise SystemExit(f'Build failed. Full log: {log}')
    return subprocess.run(args, check=True, **kwargs)


def download(source):
    downloads = CACHE / 'downloads'
    downloads.mkdir(parents=True, exist_ok=True)
    archive = downloads / source['archive']
    if not archive.exists():
        print(f'Downloading {source["archive"]}…', flush=True)
        with tempfile.NamedTemporaryFile(dir=downloads, delete=False) as partial:
            partial_path = Path(partial.name)
        try:
            urllib.request.urlretrieve(source['url'], partial_path)
            if sha(partial_path) != source['sha256']:
                raise SystemExit(f'Source checksum mismatch: {source["archive"]}')
            partial_path.replace(archive)
        finally:
            partial_path.unlink(missing_ok=True)
    if sha(archive) != source['sha256']:
        raise SystemExit(f'Source checksum mismatch: {archive}')
    return archive


def extract(source, destination):
    archive = download(source)
    if destination.exists():
        return destination
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=destination.parent) as temporary:
        with tarfile.open(archive) as tar:
            tar.extractall(temporary, filter='data')
        roots = list(Path(temporary).iterdir())
        if len(roots) != 1 or not roots[0].is_dir():
            raise SystemExit(f'Expected one source directory in {archive}')
        roots[0].rename(destination)
    return destination


def audit(worker, system, arch):
    if system == 'darwin':
        linked = subprocess.check_output(['otool', '-L', str(worker)], text=True).splitlines()[1:]
        if any(not line.strip().startswith(('/usr/lib/', '/System/Library/')) for line in linked):
            raise SystemExit('Worker depends on a non-system library:\n' + '\n'.join(linked))
        headers = subprocess.check_output(['otool', '-l', str(worker)], text=True)
        minimums = re.findall(r'\bminos\s+(\S+)', headers)
        if not minimums or any(tuple(map(int, version.split('.'))) > tuple(map(int, LOCK['minimumMacOS'].split('.'))) for version in minimums):
            raise SystemExit(f'Worker deployment target exceeds {LOCK["minimumMacOS"]}: {minimums}')
        actual_arch = subprocess.check_output(['lipo', '-archs', str(worker)], text=True).strip()
        if actual_arch != arch:
            raise SystemExit(f'Worker architecture mismatch: {actual_arch} != {arch}')
        return {'dynamicLibraries': [line.strip() for line in linked], 'minimumMacOS': minimums[0]}
    if system == 'windows':
        headers = subprocess.check_output(['objdump', '-p', str(worker)], text=True)
        linked = re.findall(r'DLL Name:\s*(\S+)', headers)
        allowed = {'kernel32.dll', 'msvcrt.dll', 'ucrtbase.dll', 'advapi32.dll', 'bcrypt.dll', 'user32.dll', 'gdi32.dll', 'ole32.dll', 'shell32.dll', 'ws2_32.dll', 'winmm.dll', 'version.dll', 'ntdll.dll', 'secur32.dll', 'crypt32.dll', 'urlmon.dll'}
        unexpected = [lib for lib in linked if lib.lower() not in allowed and not lib.lower().startswith(('api-ms-win-', 'ext-ms-win-'))]
        if unexpected:
            raise SystemExit(f'Worker requires non-system DLLs: {unexpected}')
        return {'dynamicLibraries': linked}
    headers = subprocess.check_output(['readelf', '-d', str(worker)], text=True)
    linked = re.findall(r'Shared library: \[(.*?)\]', headers)
    allowed = {'libc.so.6', 'libm.so.6', 'libpthread.so.0', 'libdl.so.2', 'librt.so.1', 'libgcc_s.so.1', 'libstdc++.so.6'}
    # glibc's loader can also appear as a direct dependency (Ubuntu ARM64).
    allowed.add({'arm64': 'ld-linux-aarch64.so.1', 'x86_64': 'ld-linux-x86-64.so.2'}[arch])
    if set(linked) - allowed:
        raise SystemExit(f'Worker requires unexpected shared libraries: {linked}')
    resolved = subprocess.check_output(['ldd', str(worker)], text=True)
    if 'not found' in resolved:
        raise SystemExit(f'Missing worker dependency:\n{resolved}')
    return {'dynamicLibraries': linked, 'libc': platform.libc_ver()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--arch', choices=['arm64', 'x86_64'], help='macOS target architecture; other systems build natively')
    args = parser.parse_args()
    host_arch = {'aarch64': 'arm64', 'amd64': 'x86_64'}.get(platform.machine().lower(), platform.machine().lower())
    system = platform.system().lower()
    if os.environ.get('MSYSTEM'):
        if os.environ['MSYSTEM'] != 'UCRT64' or os.name == 'nt':
            raise SystemExit('On Windows run /usr/bin/python scripts/prepare-image-backend.py in MSYS2 UCRT64. See BACKEND.md.')
        system = 'windows'
        host_arch = 'x86_64'
    if system not in ('darwin', 'linux', 'windows') or host_arch not in ('arm64', 'x86_64'):
        raise SystemExit(f'Unsupported build host: {system}/{host_arch}. See BACKEND.md.')
    arch = args.arch or host_arch
    if system != 'darwin' and arch != host_arch:
        raise SystemExit('Only macOS architecture cross-builds are configured. Use a native Linux/Windows runner.')
    for tool in ['cmake', 'make', 'pkg-config', 'cc', 'c++']:
        if not shutil.which(tool):
            raise SystemExit(f'Missing build tool: {tool}. See BACKEND.md.')
    fingerprint = hashlib.sha256((json.dumps(LOCK, sort_keys=True) + Path(__file__).read_text() + (ROOT / 'scripts/image-policy.xml').read_text()).encode()).hexdigest()[:12]
    work = CACHE / f'{system}-{arch}-{fingerprint}'
    work.mkdir(parents=True, exist_ok=True)
    stage = work / 'install'
    logs = work / 'logs'
    logs.mkdir(exist_ok=True)
    env = os.environ.copy()
    # Never inherit a developer's search paths or converter flags.
    for key in ['CPATH', 'C_INCLUDE_PATH', 'CPLUS_INCLUDE_PATH', 'LIBRARY_PATH', 'SDKROOT', 'CMAKE_PREFIX_PATH', 'LIBS']:
        env.pop(key, None)
    cc, cxx = ('clang', 'clang++') if system == 'darwin' else ('gcc', 'g++')
    flags = '-O2 -fPIC'
    linker = f'-L{shlex.quote(str(stage / "lib"))}'
    if system == 'darwin':
        flags += f' -arch {arch} -mmacosx-version-min={LOCK["minimumMacOS"]}'
        linker += f' -arch {arch} -mmacosx-version-min={LOCK["minimumMacOS"]}'
        env['MACOSX_DEPLOYMENT_TARGET'] = LOCK['minimumMacOS']
    elif system == 'windows':
        cc, cxx = '/ucrt64/bin/gcc', '/ucrt64/bin/g++'
        linker += ' -static -static-libgcc -static-libstdc++'
    else:
        linker += ' -static-libgcc -static-libstdc++'
    env.update(CC=cc, CXX=cxx, CFLAGS=flags, CXXFLAGS=flags,
               CPPFLAGS=f'-I{shlex.quote(str(stage / "include"))}', LDFLAGS=linker,
               PKG_CONFIG='pkg-config --static', PKG_CONFIG_PATH='', PKG_CONFIG_LIBDIR=str(stage / 'lib/pkgconfig'))
    for key in ['JPEG', 'PNG', 'WEBP', 'WEBPMUX', 'LCMS2', 'ZLIB']:
        env.pop(f'{key}_CFLAGS', None)
        env.pop(f'{key}_LIBS', None)
    options = {
        'zlib': ['ZLIB_BUILD_SHARED=OFF', 'ZLIB_BUILD_STATIC=ON', 'ZLIB_BUILD_TESTING=OFF'],
        'jpeg-turbo': ['ENABLE_SHARED=OFF', 'ENABLE_STATIC=ON', 'WITH_TURBOJPEG=OFF', 'WITH_TOOLS=OFF', 'WITH_TESTS=OFF'],
        'libpng': ['PNG_SHARED=OFF', 'PNG_STATIC=ON', 'PNG_FRAMEWORK=OFF', 'PNG_TESTS=OFF', 'PNG_TOOLS=OFF', f'ZLIB_ROOT={stage}'],
        'webp': ['BUILD_SHARED_LIBS=OFF', 'WEBP_LINK_STATIC=ON', 'WEBP_BUILD_LIBWEBPMUX=ON'] + [f'WEBP_BUILD_{name}=OFF' for name in ['ANIM_UTILS', 'CWEBP', 'DWEBP', 'GIF2WEBP', 'IMG2WEBP', 'VWEBP', 'WEBPINFO', 'WEBPMUX', 'EXTRAS']],
        'little-cms2': ['BUILD_SHARED_LIBS=OFF', 'LCMS2_BUILD_SHARED=OFF', 'LCMS2_BUILD_TOOLS=OFF', 'LCMS2_BUILD_TESTS=OFF'],
    }
    cmake_common = [f'-DCMAKE_INSTALL_PREFIX={stage}', '-DCMAKE_INSTALL_LIBDIR=lib', '-DCMAKE_BUILD_TYPE=Release', '-DCMAKE_POSITION_INDEPENDENT_CODE=ON', '-DCMAKE_FIND_FRAMEWORK=NEVER', '-DCMAKE_FIND_USE_PACKAGE_REGISTRY=OFF', f'-DCMAKE_PREFIX_PATH={stage}']
    if system == 'darwin':
        cmake_common += [f'-DCMAKE_OSX_DEPLOYMENT_TARGET={LOCK["minimumMacOS"]}', f'-DCMAKE_OSX_ARCHITECTURES={arch}']
    if system == 'windows':
        cmake_common += ['-G', 'MSYS Makefiles']
        options['libpng'] += [f'ZLIB_LIBRARY={stage / "lib/libz.a"}', f'ZLIB_INCLUDE_DIR={stage / "include"}']
    sources = {}
    for dependency in LOCK['dependencies']:
        name = dependency['name']
        source = extract(dependency, work / 'sources' / name)
        sources[name] = source
        marker = work / f'{name}.done'
        if marker.exists():
            continue
        print(f'Building {name} {dependency["version"]} for {system}/{arch}…', flush=True)
        build = work / 'build' / name
        log = logs / f'{name}.log'
        run(['cmake', '-S', str(source), '-B', str(build), *cmake_common, *['-D' + option for option in options[name]]], env=env, log=log)
        run(['cmake', '--build', str(build), '--parallel', JOBS], env=env, log=log)
        run(['cmake', '--install', str(build)], env=env, log=log)
        if system == 'windows' and name == 'zlib':
            # zlib 1.3.2 installs libzs.a on Windows but its .pc file requests -lz.
            # Supply that name from our pinned archive, never the MSYS2 copy.
            shutil.copyfile(stage / 'lib/libzs.a', stage / 'lib/libz.a')
        marker.touch()
    unexpected = [path for path in (stage / 'lib').glob('*') if path.suffix in ('.so', '.dylib', '.dll') or '.dll.a' in path.name]
    if unexpected:
        raise SystemExit(f'Delegates must be static: {unexpected}')
    source = extract(LOCK['source'], work / 'sources/ImageMagick')
    policy = (ROOT / 'scripts/image-policy.xml').read_text()
    private = source / 'MagickCore/policy-private.h'
    private.write_text(re.sub(r'\*ZeroConfigurationPolicy\s*=.*?;', lambda _: '*ZeroConfigurationPolicy = ' + json.dumps(policy) + ';', private.read_text(), flags=re.S))
    configure = [f'--prefix={stage}', '--disable-shared', '--enable-static', '--enable-zero-configuration', '--disable-installed', '--disable-hdri', '--disable-openmp', '--disable-opencl', '--disable-docs', '--disable-dpc', '--disable-cipher', '--without-modules', '--without-magick-plus-plus', '--without-perl', '--with-quantum-depth=16', '--with-security-policy=open', '--with-jpeg=yes', '--with-png=yes', '--with-webp=yes', '--with-lcms=yes']
    configure += [f'--without-{name}' for name in ['x', 'bzlib', 'zip', 'zstd', 'autotrace', 'dps', 'fftw', 'flif', 'fpx', 'djvu', 'fontconfig', 'freetype', 'raqm', 'gdi32', 'gslib', 'gvc', 'dmr', 'heic', 'jbig', 'jxl', 'openjp2', 'lqr', 'lzma', 'openexr', 'pango', 'raw', 'rsvg', 'tiff', 'uhdr', 'wmf', 'xml']]
    if system == 'windows':
        configure += ['--host=x86_64-w64-mingw32']
    if system == 'darwin' and arch != host_arch:
        configure += [f'--host={arch}-apple-darwin']
    print('Configuring and building ImageMagick…', flush=True)
    run(['sh', str(source / 'configure'), *configure], cwd=source, env=env, log=logs / 'imagemagick-configure.log')
    make = ['make', '-j', JOBS, 'V=1']
    if system == 'windows':
        # Libtool consumes -static without passing it to the final compiler.
        # -all-static also embeds compiler/pthread runtimes; retain Unicode argv.
        make += ['UTILITIES_LDFLAGS_EXTRA=-municode -all-static']
    run(make, cwd=source, env=env, log=logs / 'imagemagick-build.log')
    OUT.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.image-backend-', dir=OUT.parent) as temporary:
        output = Path(temporary)
        worker = output / ('magick.exe' if system == 'windows' else 'magick')
        shutil.copy2(source / 'utilities' / worker.name, worker)
        run(['strip', '-x' if system == 'darwin' else '--strip-unneeded', str(worker)])
        if system == 'darwin':
            run(['codesign', '--force', '--sign', '-', str(worker)])
        audited = audit(worker, system, arch)
        profile = ROOT / 'scripts/sRGB.icc'
        if sha(profile) != LOCK['profileSha256']:
            raise SystemExit('Bundled sRGB profile checksum mismatch.')
        shutil.copyfile(profile, output / 'sRGB.icc')
        shutil.copyfile(ROOT / 'scripts/image-policy.xml', output / 'policy.xml')
        notices = output / 'licenses'
        notices.mkdir()
        shutil.copyfile(source / 'LICENSE', notices / 'ImageMagick-LICENSE')
        for dependency in LOCK['dependencies']:
            for license_file in dependency['licenses']:
                shutil.copyfile(sources[dependency['name']] / license_file, notices / f'{dependency["name"]}-{Path(license_file).name}')
        version = None
        if arch == host_arch:
            version = subprocess.check_output([str(worker), '-version'], env=env, text=True)
            if not all(value in version for value in [f'ImageMagick {LOCK["source"]["version"]}', 'Zero-configuration', 'jpeg', 'png', 'webp', 'lcms']):
                raise SystemExit(f'Required image capabilities missing:\n{version}')
        manifest = {'system': system, 'architecture': arch, 'recipeFingerprint': fingerprint, 'source': LOCK['source'], 'dependencies': LOCK['dependencies'], 'version': version, 'binarySha256': sha(worker), 'profileSha256': sha(profile), **audited}
        (output / 'build-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        # Preserve the last working backend until its replacement is complete.
        backup = OUT.with_name('.image-backend-previous')
        if backup.exists():
            raise SystemExit(f'A previous backend backup needs review before replacing it: {backup}')
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
    print(version or f'Cross-built {system}/{arch}; runtime verification needs that architecture.', flush=True)
    print(f'Bundled worker: {OUT}', flush=True)


if __name__ == '__main__':
    main()
