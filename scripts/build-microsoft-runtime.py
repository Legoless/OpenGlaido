#!/usr/bin/env python3
"""Build a self-contained, offline Microsoft speech runtime on Apple Silicon."""
from pathlib import Path
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'native-microsoft'
CACHE = ROOT / 'src-tauri/target/microsoft-runtime'
DEST = ROOT / 'src-tauri/microsoft-runtime'
PYTHON = CACHE / 'venv/bin/python'
MACHO = {b'\xfe\xed\xfa\xce', b'\xce\xfa\xed\xfe', b'\xfe\xed\xfa\xcf', b'\xcf\xfa\xed\xfe', b'\xca\xfe\xba\xbe', b'\xbe\xba\xfe\xca'}

def run(args):
    subprocess.run([str(a) for a in args], cwd=ROOT, check=True)

def fingerprint():
    digest = hashlib.sha256()
    for file in sorted(SOURCE.rglob('*')):
        if file.is_file() and '__pycache__' not in file.parts and file.suffix != '.pyc':
            digest.update(str(file.relative_to(SOURCE)).encode())
            digest.update(file.read_bytes())
    digest.update(Path(__file__).read_bytes())
    return digest.hexdigest()

def sign(identity):
    if not identity:
        return
    for file in sorted(DEST.rglob('*'), key=lambda p: len(p.parts), reverse=True):
        if not file.is_file() or file.is_symlink():
            continue
        with file.open('rb') as stream:
            if stream.read(4) not in MACHO:
                continue
        run(['codesign', '--force', '--options', 'runtime', '--timestamp', '--sign', identity, file])
    for framework in sorted(DEST.rglob('*.framework'), key=lambda p: len(p.parts), reverse=True):
        if not framework.is_symlink():
            run(['codesign', '--force', '--timestamp', '--sign', identity, framework])
    run(['codesign', '--verify', '--strict', DEST / 'openglaido-microsoft'])

def main():
    if platform.system() != 'Darwin':
        print('Microsoft Python speech models are available on Apple Silicon Macs.')
        return
    if platform.machine() != 'arm64':
        DEST.mkdir(parents=True, exist_ok=True)
        (DEST / 'UNSUPPORTED.txt').write_text('Microsoft Python speech models require Apple Silicon and macOS 14 or later.\n')
        return
    os.environ['PYTHONDONTWRITEBYTECODE'] = '1'
    CACHE.mkdir(parents=True, exist_ok=True)
    stamp = CACHE / 'packaged.json'
    recipe = fingerprint()
    if not (DEST / 'openglaido-microsoft').is_file() or not stamp.is_file() or json.loads(stamp.read_text()).get('recipe') != recipe:
        if not shutil.which('uv'):
            raise SystemExit('Install uv to build the Microsoft speech runtime')
        if not PYTHON.is_file():
            run(['uv', '--no-config', 'venv', '--python', '3.12', CACHE / 'venv'])
        run(['uv', '--no-config', 'pip', 'sync', SOURCE / 'requirements.lock', '--python', PYTHON, '--require-hashes', '--only-binary', ':all:', '--default-index', 'https://pypi.org/simple'])
        run([PYTHON, SOURCE / 'helper.py', '--self-test'])
        run([PYTHON, SOURCE / 'helper.py', '--check-runtime'])
        for bytecode in (SOURCE / 'vendor').rglob('__pycache__'):
            shutil.rmtree(bytecode)
        args = [PYTHON, '-m', 'PyInstaller', '--noconfirm', '--clean', '--onedir', '--name', 'openglaido-microsoft', '--distpath', CACHE / 'dist', '--workpath', CACHE / 'work', '--specpath', CACHE, '--paths', SOURCE / 'vendor', '--add-data', f'{SOURCE / "vendor"}:vendor', '--add-data', f'{SOURCE / "THIRD-PARTY-NOTICES.txt"}:.', '--collect-submodules', 'vibevoice', '--collect-submodules', 'phi4']
        for package in ['torch', 'torchvision', 'transformers', 'peft', 'accelerate', 'diffusers', 'numpy', 'scipy', 'PIL', 'backoff']:
            args += ['--collect-all', package]
        for package in ['torch', 'torchvision', 'transformers', 'peft', 'accelerate', 'diffusers', 'numpy', 'scipy', 'pillow', 'backoff', 'pyinstaller']:
            args += ['--recursive-copy-metadata', package]
        args += [SOURCE / 'helper.py']
        run(args)
        staged = CACHE / 'dist/openglaido-microsoft'
        run([staged / 'openglaido-microsoft', '--self-test'])
        run([staged / 'openglaido-microsoft', '--check-runtime'])
        if any((staged / '_internal/vendor').rglob('__pycache__')):
            raise SystemExit('Frozen model imports must not modify signed app resources')
        if fingerprint() != recipe:
            raise SystemExit('Runtime sources changed during packaging; build again')
        if DEST.exists():
            shutil.rmtree(DEST)
        # Tauri materializes resource symlinks. Sign that physical layout, so a
        # copied Python framework alias has a valid standalone library signature.
        shutil.copytree(staged, DEST, symlinks=False)
        # The frozen bootloader loads this standalone Python library. Keeping a
        # dereferenced framework beside it creates an ambiguous signing bundle;
        # stdlib and model modules are already supplied by the frozen runtime.
        framework = DEST / '_internal/Python.framework'
        if framework.is_dir():
            if not (DEST / '_internal/Python').is_file():
                raise SystemExit('Frozen runtime is missing its standalone Python library')
            shutil.rmtree(framework)
        if any(path.is_symlink() for path in DEST.rglob('*')):
            raise SystemExit('Microsoft runtime resources must contain physical files')
        license_file = subprocess.check_output([str(PYTHON), '-c', "import pathlib,sysconfig; print(pathlib.Path(sysconfig.get_path('stdlib')) / 'LICENSE.txt')"], text=True).strip()
        shutil.copyfile(license_file, DEST / 'CPython-LICENSE.txt')
        stamp.write_text(json.dumps({'recipe': recipe}) + '\n')
    sign(os.environ.get('APPLE_SIGNING_IDENTITY'))
    run([DEST / 'openglaido-microsoft', '--self-test'])
    run([DEST / 'openglaido-microsoft', '--check-runtime'])
    if fingerprint() != recipe:
        raise SystemExit('Runtime sources changed during signing; build again')
    print(f'Microsoft speech runtime staged: {DEST}', flush=True)

if __name__ == '__main__':
    main()
