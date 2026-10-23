#!/usr/bin/env python3
"""Check the packaged Office worker using independent OOXML and PDF readers."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import zipfile
from pypdf import PdfReader
import fixtures


def check(payload, evidence):
    evidence.mkdir(parents=True, exist_ok=True)
    sources, outputs = evidence/'fixtures', evidence/'outputs'
    sources.mkdir(exist_ok=True); outputs.mkdir(exist_ok=True)
    worker = payload / json.loads((payload/'provider.json').read_text(encoding='utf-8'))['executable']
    records = []
    fixtures.write(sources/'layout.docx', fixtures.parts())
    fixtures.write(sources/'blank.docx', fixtures.parts('<w:p/>'+fixtures.section(12240,15840)))
    languages = [('Café — Ελληνικά — Русский', 'Liberation Sans'),('你好世界 — こんにちは','Noto Sans CJK SC'),('مرحبا بالعالم','Noto Naskh Arabic'),('नमस्ते दुनिया','Noto Sans Devanagari'),('🙂','Noto Emoji')]
    fixtures.write(sources/'unicode.docx', fixtures.parts(''.join(fixtures.paragraph(text,font) for text,font in languages)+fixtures.section(12240,15840)))
    fixtures.write(sources/'long.docx', fixtures.parts(''.join(fixtures.paragraph(f'Paragraph {i}. '+ 'Ordinary text. '*15) for i in range(120))+fixtures.section(12240,15840)))
    comments = fixtures.parts()
    comments['word/comments.xml'] = f'<w:comments xmlns:w="{fixtures.W}"><w:comment w:id="1" w:author="Recast">'+fixtures.paragraph('A review comment.')+'</w:comment></w:comments>'
    fixtures.write(sources/'comments.docx', comments)
    fixtures.write(sources/'résumé 你好.docx', fixtures.parts())

    def run(name, action='convert', issue=None, settings=None, cancel=False):
        source=sources/name; before=source.read_bytes(); output=outputs/(source.stem+'.pdf')
        prior=output.read_bytes() if output.exists() else None
        with tempfile.TemporaryDirectory(prefix='office request ', dir=evidence) as directory:
            work=Path(directory).resolve()
            request={'protocolVersion':1,'action':action,'source':str(source.resolve()),'workspace':str(work),'output':str(output.resolve()),'target':'pdf','settings':settings or {}}
            (work/'request.json').write_text(json.dumps(request),encoding='utf-8')
            if cancel: (work/'cancel').touch()
            start=time.monotonic()
            process=subprocess.run([str(worker),'--request',str(work/'request.json')],capture_output=True,text=True,timeout=330)
            response=json.loads((work/'response.json').read_text(encoding='utf-8'))
            records.append({'file':name,'action':action,'seconds':round(time.monotonic()-start,3),'response':response})
            (evidence/'report.json').write_text(json.dumps(records,indent=2)+'\n',encoding='utf-8')
            assert source.read_bytes()==before
            if issue:
                assert process.returncode!=0 and issue.lower() in response.get('error','').lower(),response
                assert (output.read_bytes() if output.exists() else None)==prior
                return
            if process.returncode:
                for diagnostic in ('office.log','response.json'):
                    if (work/diagnostic).exists(): shutil.copyfile(work/diagnostic,evidence/(source.stem+'-'+diagnostic))
            assert process.returncode==0 and not response.get('error'),(name,response,process.stderr)
            if action=='inspect':
                assert response['input']=='docx' and response['targets']==['pdf'];return
            pdf=PdfReader(output,strict=True);assert not pdf.is_encrypted
            text='\n'.join(page.extract_text() for page in pdf.pages)
            output.with_suffix('.txt').write_text(text,encoding='utf-8')
            if name=='layout.docx':
                assert len(pdf.pages)==2,len(pdf.pages)
                for page,(w,h) in zip(pdf.pages,[(419.55,595.3),(792,612)]):
                    assert abs(float(page.mediabox.width)-w)<.15 and abs(float(page.mediabox.height)-h)<.15,page.mediabox
                    assert 'RECAST HEADER' in page.extract_text() and 'RECAST FOOTER' in page.extract_text()
                for word in ['Portrait section','Landscape section','Paper','12','Footnote retained.','Working hyperlink']: assert word in text,(word,text)
                assert 'E=mc2' in ''.join(text.split()), text
                assert any('/Image'==obj.get_object().get('/Subtype') for page in pdf.pages for obj in page['/Resources'].get('/XObject',{}).get_object().values())
                assert any(a.get_object().get('/A',{}).get('/URI')=='https://example.com/' or a.get_object().get('/A',{}).get('/URI')=='https://example.com' for page in pdf.pages for a in page.get('/Annots',[]))
            elif name=='blank.docx': assert len(pdf.pages)==1
            elif name=='long.docx': assert len(pdf.pages)>3 and 'Paragraph 119.' in text
            elif name=='unicode.docx':
                for expected in ['Café','Ελληνικά','Русский','你好世界','こんにちは','🙂']: assert expected in text,(expected,text)
                assert '\ufffd' not in text
            elif name=='comments.docx': assert response['warnings']==['Document comments are not included in the PDF.']
        return output

    for name in ['layout.docx','blank.docx','unicode.docx','long.docx','comments.docx','résumé 你好.docx']:
        run(name,'inspect');run(name)
    run('layout.docx',settings={'page-size':'A4'},issue='original layout')
    run('layout.docx',cancel=True,issue='Cancelled')
    for name,data,issue in [
        ('not-docx.docx',b'not a zip','readable DOCX'),('encrypted.docx',b'\xd0\xcf\x11\xe0'+b'\0'*20,'Encrypted'),
    ]:
        (sources/name).write_bytes(data);run(name,'inspect',issue)
    for name,mutate,issue in [
        ('truncated',lambda d:d.update({'word/document.xml':d['word/document.xml'][:-14]}),'XML'),
        ('multiple-roots',lambda d:d.update({'word/document.xml':d['word/document.xml']+'<extra/>'}),'XML'),
        ('missing-image',lambda d:d.pop('word/media/strip.png'),'missing a referenced'),
        ('macro',lambda d:d.update({'word/vbaProject.bin':b'bad'}),'Macros'),
        ('external-image',lambda d:d.update({'word/_rels/document.xml.rels':d['word/_rels/document.xml.rels'].replace('Target="media/strip.png"','Target="https://example.com/strip.png" TargetMode="External"')}),'external image'),
        ('tracked',lambda d:d.update({'word/document.xml':d['word/document.xml'].replace('<w:body>','<w:body><w:ins w:id="1"/>')}),'tracked changes'),
        ('tracked-format',lambda d:d.update({'word/document.xml':d['word/document.xml'].replace('<w:body>','<w:body><w:p><w:pPr><w:pPrChange w:id="1"/></w:pPr></w:p>')}),'tracked changes'),
        ('embedded',lambda d:d.update({'word/embeddings/object.bin':b'embedded'}),'embedded'),
        ('field',lambda d:d.update({'word/document.xml':d['word/document.xml'].replace('<w:body>','<w:body><w:p><w:r><w:instrText>INCLUDETEXT &quot;https://example.com&quot;</w:instrText></w:r></w:p>')}),'external fields'),
        ('entity',lambda d:d.update({'word/document.xml':'<!DOCTYPE document [<!ENTITY ex SYSTEM "file:///missing">]>'+d['word/document.xml']}),'XML entities'),
        ('escaping-part',lambda d:d.update({'../outside.xml':'<root/>'}),'unsupported'),
    ]:
        data=fixtures.parts();mutate(data);fixtures.write(sources/(name+'.docx'),data);run(name+'.docx','inspect',issue)
    # Cancel a real child or mutate the original while its snapshot renders.
    # Neither case may replace an existing output.
    for operation in ['cancel', 'source-change']:
      with tempfile.TemporaryDirectory(prefix=operation+' ',dir=evidence) as directory:
        work=Path(directory).resolve();output=work/'output.pdf';output.write_bytes(b'existing')
        source=work/'original.docx';shutil.copyfile(sources/'long.docx',source)
        request={'protocolVersion':1,'action':'convert','source':str(source),'workspace':str(work),'output':str(output),'target':'pdf','settings':{}}
        (work/'request.json').write_text(json.dumps(request),encoding='utf-8')
        process=subprocess.Popen([str(worker),'--request',str(work/'request.json')],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        try:
            deadline=time.monotonic()+15
            while not (work/'office.log').exists() and process.poll() is None and time.monotonic()<deadline: time.sleep(.01)
            assert (work/'office.log').exists(),'Engine never started'
            if operation=='cancel':
                time.sleep(.15);(work/'cancel').touch()
            else: source.write_bytes(b'changed during conversion')
            assert process.wait(timeout=45)!=0
            assert output.read_bytes()==b'existing'
            issue=json.loads((work/'response.json').read_text(encoding='utf-8'))['error']
            assert ('Cancelled' if operation=='cancel' else 'changed during conversion') in issue,issue
            records.append({'action':operation,'passed':True})
        finally:
            if process.poll() is None: process.kill();process.wait()
    (evidence/'report.json').write_text(json.dumps(records,indent=2)+'\n',encoding='utf-8')
    print(f'{len(records)} packaged Office checks passed.')

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    source=parser.add_mutually_exclusive_group(required=True)
    source.add_argument('--payload',type=Path);source.add_argument('--package',type=Path)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    if args.payload: check(args.payload.resolve(),args.output.resolve())
    else:
        sys.path.insert(0,str(Path(__file__).resolve().parent.parent/'shared'))
        import package
        archives=list(args.package.glob('*.recast-extension')) if args.package.is_dir() else [args.package]
        assert len(archives)==1,archives
        manifest=package.verify(archives[0])
        with tempfile.TemporaryDirectory(prefix='recast-office-package-') as directory:
            root=Path(directory)
            with zipfile.ZipFile(archives[0]) as archive:
                for entry in manifest['files']:
                    path=root/entry['path'];path.parent.mkdir(parents=True,exist_ok=True)
                    with archive.open('payload/'+entry['path']) as src,path.open('wb') as dest: shutil.copyfileobj(src,dest)
                    path.chmod(0o755 if entry['executable'] else 0o644)
            check(root,args.output.resolve())
