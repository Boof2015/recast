#!/usr/bin/env python3
"""Exercise the packaged worker; independently inspect PDF and DOCX outputs."""
import argparse
import struct
import zlib
import sys
import json
from pathlib import Path
import subprocess
import tempfile
import time
import xml.etree.ElementTree as ET
import zipfile
from pypdf import PdfReader

def chunk(kind, data):
    return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))

PNG = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 1, 1, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(b'\0\x20\x80\xc0')) + chunk(b'IEND', b'')

W = '{http://schemas.openxmlformats.org/wordprocessingml/2006/main}'


def check(payload, evidence):
    evidence.mkdir(parents=True, exist_ok=True)
    fixtures = evidence / 'fixtures'
    fixtures.mkdir(exist_ok=True)
    outputs = evidence / 'outputs'
    outputs.mkdir(exist_ok=True)
    (fixtures / 'pixel.png').write_bytes(PNG)
    rich = '''---
title: Recast document
---

# First section

Ordinary **bold** and *italic* text. A [link](https://example.com).

- One item
- Second item
  - Nested item

| Name | Value |
|:-----|------:|
| Alpha | 42 |

> A short quotation.

```python
print("Recast")
```

Math: $x^2 + y^2 = z^2$.

A footnote.[^note]

[^note]: Footnote content.

![A local image](pixel.png)
'''
    (fixtures / 'ordinary.md').write_text(rich, encoding='utf-8')
    (fixtures / 'literal.txt').write_text('# Literal heading\n**not bold**\n<tags stay visible>\n\nSecond paragraph.', encoding='utf-8')
    (fixtures / 'unicode.md').write_text('# Languages\n\nCafé — Ελληνικά — Русский\n\n你好 世界\n\nمرحبا بالعالم\n\nनमस्ते दुनिया\n\n🙂', encoding='utf-8')
    (fixtures / 'long.md').write_text('# A longer document\n\n' + '\n\n'.join(f'Paragraph {i}. ' + 'Readable text with ordinary spacing. ' * 12 for i in range(80)), encoding='utf-8')
    descriptor = json.loads((payload / 'provider.json').read_text(encoding='utf-8')) if (payload / 'provider.json').exists() else {'executable': 'bin/recast-text-worker' + ('.exe' if __import__('os').name == 'nt' else '')}
    worker = payload / descriptor['executable']
    records = []

    def run(name, target='pdf', paper='A4', action='convert', cancel=False, expected_error=None):
        source = (fixtures / name).resolve()
        output = outputs / f'{source.stem}-{paper}.{target}'
        before = source.read_bytes()
        previous_output = output.read_bytes() if output.exists() else None
        with tempfile.TemporaryDirectory(prefix='request ', dir=evidence) as directory:
            work = Path(directory).resolve()
            request = {'protocolVersion':1,'action':action,'source':str(source),'workspace':str(work), 'output':str(output.resolve()),'target':target,'settings':{'page-size':paper}}
            (work / 'request.json').write_text(json.dumps(request), encoding='utf-8')
            if cancel:
                (work / 'cancel').touch()
            started = time.monotonic()
            process = subprocess.run([str(worker.resolve()), '--request', str(work / 'request.json')], capture_output=True, text=True, timeout=90)
            response = json.loads((work / 'response.json').read_text(encoding='utf-8'))
            records.append({'file':name,'target':target,'paper':paper,'action':action,'seconds':round(time.monotonic()-started,3),'response':response})
            if expected_error:
                assert process.returncode != 0, (name, response)
                assert expected_error.lower() in response['error'].lower(), response
                assert source.read_bytes() == before
                assert (output.read_bytes() if output.exists() else None) == previous_output
                return
            assert process.returncode == 0 and not response.get('error'), (name, response, process.stderr)
            assert source.read_bytes() == before
            if action == 'inspect':
                assert set(response['targets']) == {'pdf','docx'}
                return
            assert output.stat().st_size > 200
            if target == 'pdf':
                pdf = PdfReader(output, strict=True)
                assert len(pdf.pages) >= (3 if name == 'long.md' else 1)
                width, height = (595.276, 841.89) if paper == 'A4' else (612, 792)
                assert abs(float(pdf.pages[0].mediabox.width)-width)<0.1
                assert abs(float(pdf.pages[0].mediabox.height)-height)<0.1
                text = '\n'.join(page.extract_text() for page in pdf.pages)
                (output.with_suffix('.txt')).write_text(text, encoding='utf-8')
                if name == 'ordinary.md':
                    for expected in ['Recast document','First section','Alpha','42','Footnote content','A local image']:
                        assert expected in text, (expected, text)
                    assert any(annotation.get_object().get('/A', {}).get('/URI') == 'https://example.com' for page in pdf.pages for annotation in page.get('/Annots', []))
                if name == 'literal.txt':
                    for expected in ['# Literal heading','**not bold**','<tags stay visible>']:
                        assert expected in text, text
            else:
                with zipfile.ZipFile(output) as document:
                    assert document.testzip() is None
                    xml = ET.fromstring(document.read('word/document.xml'))
                    text = ''.join(node.text or '' for node in xml.iter(W+'t'))
                    page = xml.find('.//'+W+'pgSz')
                    assert page is not None
                    assert page.attrib[W+'w'] == ('11906' if paper == 'A4' else '12240')
                    if name == 'ordinary.md':
                        assert 'Recast document' in text and 'Alpha' in text and '42' in text
                        assert any(name.startswith('word/media/') for name in document.namelist())
                        assert b'Footnote content' in document.read('word/footnotes.xml')
                        math = '{http://schemas.openxmlformats.org/officeDocument/2006/math}'
                        assert ''.join(node.text or '' for node in xml.iter(math+'t')) == 'x2+y2=z2'
                    if name == 'unicode.md':
                        for expected in ['Café', 'Ελληνικά', 'Русский', '你好 世界', 'مرحبا بالعالم', 'नमस्ते दुनिया', '🙂']:
                            assert expected in text, (expected, text)
                    if name == 'literal.txt':
                        assert '# Literal heading' in text and '**not bold**' in text and '<tags stay visible>' in text
        return output

    for name in ['ordinary.md','literal.txt','unicode.md']:
        run(name, action='inspect')
        for target in ['pdf','docx']:
            for paper in ['A4','Letter']:
                run(name,target,paper)
    run('long.md')
    for name, content, issue in [
        ('missing.md','![Missing](absent.png)','absent.png'),
        ('remote.md','![Remote](https://example.com/image.png)','Remote images'),
        ('raw.md','```{=typst}\n#read("/private-data")\n```','Raw HTML'),
        ('metadata.md','---\nheader-includes: hello\n---\nText','header-includes'),
        ('unknown-glyph.md','Missing glyph: \U0002FA1E','cannot render'),
    ]:
        (fixtures / name).write_text(content, encoding='utf-8')
        run(name, expected_error=issue)
    (fixtures / 'binary.txt').write_bytes(b'\x00binary\x00')
    run('binary.txt', action='inspect', expected_error='binary data')
    (fixtures / 'utf16.txt').write_bytes('Hello'.encode('utf-16'))
    run('utf16.txt', action='inspect', expected_error='UTF-8')
    run('ordinary.md', cancel=True, expected_error='Cancelled')
    (fixtures / 'attribute.md').write_text('![Image](pixel.png){typst:width="10pt"}', encoding='utf-8')
    run('attribute.md', expected_error='Custom Typst')
    (fixtures / 'literal-prefix.txt').write_text('typst: is literal text.', encoding='utf-8')
    run('literal-prefix.txt')
    # Cancel after the worker has begun loading/parsing, and reap it even if
    # an assertion fails. No partial output may replace an existing file.
    with tempfile.TemporaryDirectory(prefix='cancel ', dir=evidence) as directory:
        work = Path(directory).resolve()
        output = work / 'existing.pdf'
        output.write_bytes(b'existing output')
        request = {'protocolVersion':1,'action':'convert','source':str((fixtures/'long.md').resolve()),'workspace':str(work),'output':str(output),'target':'pdf','settings':{'page-size':'A4'}}
        (work/'request.json').write_text(json.dumps(request), encoding='utf-8')
        process = subprocess.Popen([str(worker.resolve()), '--request', str(work/'request.json')], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            deadline = time.monotonic() + 15
            while not (work/'progress.json').exists() and process.poll() is None and time.monotonic() < deadline:
                time.sleep(.01)
            assert (work/'progress.json').exists(), 'Worker did not begin conversion'
            (work/'cancel').touch()
            assert process.wait(timeout=15) != 0
            assert output.read_bytes() == b'existing output'
            response = json.loads((work/'response.json').read_text(encoding='utf-8'))
            assert response.get('error'), response
            records.append({'action':'in-flight cancellation','response':response})
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()

    (evidence / 'report.json').write_text(json.dumps(records,indent=2,ensure_ascii=False)+'\n',encoding='utf-8')
    print(f'{len(records)} packaged document checks passed.')

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    inputs = parser.add_mutually_exclusive_group(required=True)
    inputs.add_argument('--payload',type=Path)
    inputs.add_argument('--package',type=Path)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    if args.payload:
        check(args.payload.resolve(), args.output.resolve())
    else:
        sys.path.insert(0, str(Path(__file__).resolve().parent.parent / 'shared'))
        import package
        archives = list(args.package.glob('*.recast-extension')) if args.package.is_dir() else [args.package]
        assert len(archives) == 1, archives
        manifest = package.verify(archives[0])
        with tempfile.TemporaryDirectory(prefix='recast-text-package-') as directory:
            root = Path(directory)
            with zipfile.ZipFile(archives[0]) as archive:
                for entry in manifest['files']:
                    path = root / entry['path']
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(archive.read('payload/' + entry['path']))
                    path.chmod(0o755 if entry['executable'] else 0o644)
            check(root, args.output.resolve())
