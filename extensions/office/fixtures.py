"""Independent OOXML fixtures: no Office installation is used to generate these."""
import struct
import zlib
import zipfile
from html import escape

W = 'http://schemas.openxmlformats.org/wordprocessingml/2006/main'
R = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'
P = 'http://schemas.openxmlformats.org/package/2006/relationships'
T = 'http://schemas.openxmlformats.org/package/2006/content-types'

def paragraph(text, font='Liberation Sans'):
    return f'<w:p><w:r><w:rPr><w:rFonts w:ascii="{font}" w:hAnsi="{font}" w:eastAsia="{font}" w:cs="{font}"/><w:sz w:val="24"/></w:rPr><w:t>{escape(text)}</w:t></w:r></w:p>'

def section(width, height, landscape=False):
    return f'<w:sectPr><w:headerReference w:type="default" r:id="header"/><w:footerReference w:type="default" r:id="footer"/><w:type w:val="nextPage"/><w:pgSz w:w="{width}" w:h="{height}"' + (' w:orient="landscape"' if landscape else '') + '/><w:pgMar w:top="720" w:right="720" w:bottom="720" w:left="720" w:header="300" w:footer="300"/></w:sectPr>'

def png():
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind+data))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 120, 24, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(b''.join(b'\0' + b'\x30\x80\xb0'*40 + b'\xa0\xc0\x80'*40 + b'\xe0\xa0\x40'*40 for _ in range(24)))) + chunk(b'IEND', b'')

def parts(body=None):
    image = '''<w:p><w:r><w:drawing><wp:inline><wp:extent cx="2286000" cy="457200"/><wp:docPr id="1" name="Color strip"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="1" name="Color strip"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="image"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2286000" cy="457200"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>'''
    if body is None:
        body = paragraph('Portrait section') + paragraph('Original A5 layout. Café and ordinary text.')
        body += '<w:tbl><w:tblPr><w:tblW w:w="6000" w:type="dxa"/></w:tblPr><w:tblGrid><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/></w:tblGrid>'
        for row in [('Item', 'Quantity'), ('Paper', '12'), ('Ink', '3')]:
            body += '<w:tr>' + ''.join('<w:tc><w:tcPr><w:tcW w:w="3000" w:type="dxa"/></w:tcPr>'+paragraph(cell)+'</w:tc>' for cell in row)+'</w:tr>'
        body += '</w:tbl>' + image + '<w:p><w:hyperlink r:id="link"><w:r><w:t>Working hyperlink</w:t></w:r></w:hyperlink></w:p>'
        body += '<w:p><w:r><w:t>Note</w:t><w:footnoteReference w:id="1"/></w:r></w:p>'
        body += '<w:p><m:oMath><m:r><m:t>E = m</m:t></m:r><m:sSup><m:e><m:r><m:t>c</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup></m:oMath></w:p>'
        body += '<w:p><w:pPr>'+section(8391,11906)+'</w:pPr></w:p>' + paragraph('Landscape section') + paragraph('Original Letter landscape layout.') + section(15840,12240,True)
    document = f'<w:document xmlns:w="{W}" xmlns:r="{R}" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture" xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math"><w:body>{body}</w:body></w:document>'
    types = f'<Types xmlns="{T}"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/>'
    for name, suffix in [('document','document.main'),('header','header'),('footer','footer'),('footnotes','footnotes')]:
        types += f'<Override PartName="/word/{name}.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.{suffix}+xml"/>'
    rels = f'<Relationships xmlns="{P}">'
    for name, target, kind in [('header','header.xml','header'),('footer','footer.xml','footer'),('image','media/strip.png','image'),('footnotes','footnotes.xml','footnotes')]:
        rels += f'<Relationship Id="{name}" Type="{R}/{kind}" Target="{target}"/>'
    rels += f'<Relationship Id="link" Type="{R}/hyperlink" Target="https://example.com" TargetMode="External"/></Relationships>'
    return {'[Content_Types].xml':types+'</Types>', '_rels/.rels':f'<Relationships xmlns="{P}"><Relationship Id="main" Type="{R}/officeDocument" Target="word/document.xml"/></Relationships>',
        'word/document.xml':document, 'word/_rels/document.xml.rels':rels,
        'word/header.xml':f'<w:hdr xmlns:w="{W}">'+paragraph('RECAST HEADER')+'</w:hdr>',
        'word/footer.xml':f'<w:ftr xmlns:w="{W}">'+paragraph('RECAST FOOTER')+'</w:ftr>',
        'word/footnotes.xml':f'<w:footnotes xmlns:w="{W}"><w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:footnote><w:footnote w:id="1">'+paragraph('Footnote retained.')+'</w:footnote></w:footnotes>',
        'word/media/strip.png':png()}

def write(path, data):
    with zipfile.ZipFile(path, 'w', zipfile.ZIP_DEFLATED) as archive:
        for name, content in data.items(): archive.writestr(name, content)


def rtf_text(value):
    result = []
    for unit in struct.unpack('<' + 'H' * (len(value.encode('utf-16-le')) // 2), value.encode('utf-16-le')):
        if unit in (92, 123, 125): result.append('\\' + chr(unit))
        elif 32 <= unit < 127: result.append(chr(unit))
        else: result.append('\\u' + str(unit if unit < 32768 else unit - 65536) + '?')
    return ''.join(result)


def rtf(body=None):
    """Independent RTF layout; also the source used for checked-in DOC fixtures."""
    header = r'{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0 Liberation Sans;}}\uc1\f0\fs24\paperw8391\paperh11906\margl720\margr720\margt720\margb720\headery300\footery300\sectd\pgwsxn8391\pghsxn11906'
    header += r'{\header\pard RECAST HEADER\par}{\footer\pard RECAST FOOTER\par}'
    if body is None:
        body = r'\pard Portrait section\par Original A5 layout. ' + rtf_text('Café and ordinary text.') + r'\par '
        for a,b in [('Item','Quantity'),('Paper','12'),('Ink','3')]:
            body += r'\trowd\cellx3000\cellx6000\intbl ' + a + r'\cell ' + b + r'\cell\row '
        body += r'\pard{\pict\pngblip\picw120\pich24\picwgoal3600\pichgoal720 ' + png().hex() + r'}\par '
        body += r'{\field{\*\fldinst HYPERLINK "https://example.com"}{\fldrslt Working hyperlink}}\par '
        body += r'Note\chftn{\footnote\pard\plain\f0\fs20 Footnote retained.}\par '
        body += r'\sect\sectd\sbkpage\lndscpsxn\pgwsxn15840\pghsxn12240\marglsxn720\margrsxn720\margtsxn720\margbsxn720\headery300\footery300\pard Landscape section\par Original Letter landscape layout.\par '
    return (header + body + '}').encode('ascii')


ODT_NS = 'xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/"'


def odt(body=None):
    """Minimal ODF 1.3 package generated without a document editor."""
    styles = f'<office:document-styles {ODT_NS} office:version="1.3"><office:styles><style:default-style style:family="paragraph"><style:text-properties fo:font-family="Liberation Sans" fo:font-size="12pt"/></style:default-style></office:styles><office:automatic-styles>'
    for name,w,h in [('A5','14.8cm','21cm'),('Wide','27.94cm','21.59cm')]:
        styles += f'<style:page-layout style:name="{name}"><style:page-layout-properties fo:page-width="{w}" fo:page-height="{h}" fo:margin="1.27cm" style:print-orientation="'+('landscape' if name=='Wide' else 'portrait')+'"/><style:header-style><style:header-footer-properties fo:min-height="0.5cm"/></style:header-style><style:footer-style><style:header-footer-properties fo:min-height="0.5cm"/></style:footer-style></style:page-layout>'
    styles += '</office:automatic-styles><office:master-styles>'
    for name,layout in [('Portrait','A5'),('Landscape','Wide')]:
        styles += f'<style:master-page style:name="{name}" style:page-layout-name="{layout}"><style:header><text:p>RECAST HEADER</text:p></style:header><style:footer><text:p>RECAST FOOTER</text:p></style:footer></style:master-page>'
    styles += '</office:master-styles></office:document-styles>'
    if body is None:
        body = '<text:p text:style-name="First">Portrait section</text:p><text:p>Original A5 layout. Café and ordinary text.</text:p><table:table table:name="Supplies"><table:table-column table:number-columns-repeated="2"/>'
        for row in [('Item','Quantity'),('Paper','12'),('Ink','3')]:
            body += '<table:table-row>'+''.join('<table:table-cell office:value-type="string"><text:p>'+cell+'</text:p></table:table-cell>' for cell in row)+'</table:table-row>'
        body += '</table:table><text:p><draw:frame text:anchor-type="as-char" svg:width="6.35cm" svg:height="1.27cm"><draw:image xlink:href="Pictures/strip.png" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"/></draw:frame></text:p>'
        body += '<text:p><text:a xlink:href="https://example.com" xlink:type="simple">Working hyperlink</text:a></text:p><text:p>Note<text:note text:id="footnote" text:note-class="footnote"><text:note-citation>1</text:note-citation><text:note-body><text:p>Footnote retained.</text:p></text:note-body></text:note></text:p>'
        body += '<text:p><draw:frame text:anchor-type="as-char" svg:width="2cm" svg:height="0.7cm"><draw:object xlink:href="./Object1" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"/></draw:frame></text:p>'
        body += '<text:p text:style-name="NewSection">Landscape section</text:p><text:p>Original Letter landscape layout.</text:p>'
    content = f'<office:document-content {ODT_NS} office:version="1.3"><office:automatic-styles><style:style style:name="First" style:family="paragraph" style:master-page-name="Portrait"/><style:style style:name="NewSection" style:family="paragraph" style:master-page-name="Landscape"><style:paragraph-properties fo:break-before="page"/></style:style></office:automatic-styles><office:body><office:text>{body}</office:text></office:body></office:document-content>'
    formula = '<math xmlns="http://www.w3.org/1998/Math/MathML"><semantics><mrow><mi>E</mi><mo>=</mo><mi>m</mi><msup><mi>c</mi><mn>2</mn></msup></mrow><annotation encoding="StarMath 5.0">E = m c^2</annotation></semantics></math>'
    manifest = '<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3">'
    for name,mime in [('/','application/vnd.oasis.opendocument.text'),('content.xml','text/xml'),('styles.xml','text/xml'),('Pictures/strip.png','image/png'),('Object1/','application/vnd.oasis.opendocument.formula'),('Object1/content.xml','text/xml')]:
        manifest += f'<manifest:file-entry manifest:full-path="{name}" manifest:media-type="{mime}"/>'
    return {'mimetype':'application/vnd.oasis.opendocument.text', 'META-INF/manifest.xml':manifest+'</manifest:manifest>', 'content.xml':content, 'styles.xml':styles, 'Pictures/strip.png':png(), 'Object1/content.xml':formula}
