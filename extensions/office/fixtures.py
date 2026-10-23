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
