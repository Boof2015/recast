#!/usr/bin/env python3
"""Verify an example package and exercise its actual worker in temporary folders."""
import argparse
from html.parser import HTMLParser
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'shared'))
import package


class Page(HTMLParser):
    def __init__(self, html):
        super().__init__(convert_charrefs=True)
        self.tags, self.content = [], ''
        self.in_code = False
        self.feed(html)

    def handle_starttag(self, tag, attrs):
        self.tags.append(tag)
        if tag == 'code':
            self.in_code = True

    def handle_endtag(self, tag):
        if tag == 'code':
            self.in_code = False

    def handle_data(self, data):
        if self.in_code:
            self.content += data


def check(archive):
    manifest = package.verify(archive)
    assert manifest['package']['id'] == 'example.text-html'
    with tempfile.TemporaryDirectory(prefix='recast-example-check-') as temporary:
        root = Path(temporary).resolve()
        with zipfile.ZipFile(archive) as bundle:
            for entry in manifest['files']:
                path = root / 'payload' / entry['path']
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(bundle.read('payload/' + entry['path']))
                path.chmod(0o755 if entry['executable'] else 0o644)
        descriptor = json.loads((root / 'payload/provider.json').read_text())
        worker = root / 'payload' / descriptor['executable']
        sample = '\nLiteral <script>alert("hello")</script> & café 日本語\n\tIndented\n'
        source = root / 'quotes & café.txt'
        source.write_text(sample, encoding='utf-8')

        def request(action, *, theme='Light', cancel=False, source_path=source, **changes):
            workspace = Path(tempfile.mkdtemp(dir=root)).resolve()
            output = workspace / '.recast-output.html'
            output.touch()  # The real host pre-creates a temporary output too.
            data = {'protocolVersion': 1, 'action': action, 'source': str(source_path), 'workspace': str(workspace)}
            if action == 'convert':
                data.update(target='html', settings={'theme': theme}, output=str(output))
            data.update(changes)
            if cancel:
                (workspace / 'cancel').touch()
            path = workspace / 'request.json'
            path.write_text(json.dumps(data), encoding='utf-8')
            process = subprocess.run([str(worker), '--request', str(path)], cwd=workspace, stdin=subprocess.DEVNULL,
                                     capture_output=True, timeout=10)
            response = json.loads((workspace / 'response.json').read_text(encoding='utf-8'))
            assert response['protocolVersion'] == 1
            assert bool(process.returncode) == ('error' in response), process.stderr
            return response, output

        response, _ = request('inspect')
        assert response == {'protocolVersion': 1, 'input': 'text', 'targets': ['html']}
        for theme in ('Light', 'Dark'):
            response, output = request('convert', theme=theme)
            assert 'error' not in response, response
            html = output.read_text(encoding='utf-8')
            parsed = Page(html)
            assert parsed.content == sample
            assert 'script' not in parsed.tags and 'img' not in parsed.tags
            assert f'color-scheme: {theme.lower()}' in html
            assert json.loads((output.parent / 'progress.json').read_text())['fraction'] == 1
            assert source.read_text(encoding='utf-8') == sample
        # Omitted settings use the descriptor default.
        response, output = request('convert', settings={})
        assert 'error' not in response and 'color-scheme: light' in output.read_text(encoding='utf-8')
        for changes in ({'theme': 'Unknown'}, {'target': 'pdf'}, {'settings': {'extra': 'value'}},
                        {'protocolVersion': 2}, {'cancel': True}):
            response, output = request('convert', **changes)
            assert 'error' in response and output.stat().st_size == 0, response
        response, _ = request('convert', output=str(source))
        assert 'error' in response and source.read_text(encoding='utf-8') == sample
        for content in (b'\xff\xfe', b'hello\x00binary', b'x' * (1024 * 1024 + 1)):
            invalid = root / 'invalid.txt'
            invalid.write_bytes(content)
            for action in ('inspect', 'convert'):
                response, output = request(action, source_path=invalid)
                assert 'error' in response and output.stat().st_size == 0, response
        for content in (b'', b'\xef\xbb\xbfHello'):
            valid = root / 'valid.txt'
            valid.write_bytes(content)
            response, output = request('convert', source_path=valid)
            assert 'error' not in response
            assert Page(output.read_text(encoding='utf-8')).content == content.decode('utf-8-sig')
    print('Example package passed: inspection, themes, literal HTML, Unicode, defaults, validation, cancellation and source preservation.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive', type=Path)
    check(parser.parse_args().archive.resolve())
