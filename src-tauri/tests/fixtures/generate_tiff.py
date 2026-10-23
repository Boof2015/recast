"""Recreate synthetic TIFF fixtures without using the converter under test."""
from pathlib import Path
import struct
import zlib

ROOT = Path(__file__).parent
W, H = 32, 20
RGB = bytes(c for y in range(H) for x in range(W) for c in (8*x, 12*y, 5*(x+y) % 256))
RGBA = bytes(c for y in range(H) for x in range(W) for c in (8*x, 12*y, 5*(x+y) % 256, [0, 128, 255][x % 3]))


def tiff(name, *, endian='<', big=False, pixels=RGB, channels=3, bits=8,
         photo=2, compression=1, planar=False, tiled=False, tags=None):
    pack = lambda fmt, *values: struct.pack(endian + fmt, *values)
    offset_type, ptr, count_fmt, entry_fmt = (16, 8, 'Q', 'HHQ') if big else (4, 4, 'H', 'HHI')
    base = {
        256: (4, [W]), 257: (4, [H]), 258: (3, [bits]*channels),
        259: (3, [compression]), 262: (3, [photo]), 277: (3, [channels]),
        284: (3, [2 if planar else 1]), 339: (3, [1]*channels),
    }
    if channels == 4 and photo == 2:
        base[338] = (3, [2])
    if tiled:
        chunks = []
        for top in range(0, H, 16):
            for left in range(0, W, 16):
                chunks.append(bytes(c for y in range(top, top+16) for x in range(left, left+16)
                                    for c in ((8*x, 12*y, 5*(x+y) % 256) if y < H else (0, 0, 0))))
        base.update({322: (4, [16]), 323: (4, [16])})
        offset_tag, size_tag = 324, 325
    elif planar:
        chunks = [pixels[c::channels] for c in range(channels)]
        base[278] = (4, [H])
        offset_tag, size_tag = 273, 279
    else:
        chunks = [pixels]
        base[278] = (4, [H])
        offset_tag, size_tag = 273, 279
    if compression == 8:
        chunks = [zlib.compress(chunk) for chunk in chunks]
    elif compression == 32773:
        # PackBits literal runs, with each scanline encoded separately.
        row_bytes = W * channels * bits // 8
        chunks = [b''.join(bytes([len(row)-1]) + row for row in
                           (chunk[i:i+row_bytes] for i in range(0, len(chunk), row_bytes))) for chunk in chunks]
    base[offset_tag] = (offset_type, [0]*len(chunks))
    base[size_tag] = (offset_type, [len(chunk) for chunk in chunks])
    base.update(tags or {})
    header_size = 16 if big else 8
    directory_size = (8 if big else 2) + len(base)*(20 if big else 12) + ptr
    data = bytearray(b'\0' * (header_size + directory_size))
    entries = []
    formats = {1: 'B', 2: 'B', 3: 'H', 4: 'I', 7: 'B', 16: 'Q'}
    fix_offsets = None
    for tag, (typ, values) in sorted(base.items()):
        raw = pack(str(len(values)) + formats[typ], *values)
        entry = pack(entry_fmt, tag, typ, len(values))
        if len(raw) <= ptr:
            location = header_size + (8 if big else 2) + len(entries)*(20 if big else 12) + (12 if big else 8)
            entry += raw.ljust(ptr, b'\0')
        else:
            data.extend(b'\0' * (-len(data) % ptr))
            location = len(data)
            entry += pack('Q' if big else 'I', location)
            data.extend(raw)
        if tag == offset_tag:
            fix_offsets = location
        entries.append(entry)
    offsets = []
    for chunk in chunks:
        data.extend(b'\0' * (-len(data) % ptr))
        offsets.append(len(data))
        data.extend(chunk)
    marker = b'II' if endian == '<' else b'MM'
    header = marker + (pack('HHHQ', 43, 8, 0, header_size) if big else pack('HI', 42, header_size))
    data[:header_size] = header
    data[header_size:header_size+directory_size] = pack(count_fmt, len(base)) + b''.join(entries) + b'\0'*ptr
    data[fix_offsets:fix_offsets+ptr*len(offsets)] = pack(('Q' if big else 'I')*len(offsets), *offsets)
    (ROOT / name).write_bytes(data)


if __name__ == '__main__':
    tiff('rgb-le.tiff')
    tiff('rgb-be.tiff', endian='>')
    tiff('rgba.tiff', pixels=RGBA, channels=4)
    tiff('associated.tiff', channels=4,
         pixels=bytes(c for i in range(0, len(RGBA), 4)
                      for c in (*[round(v*RGBA[i+3]/255) for v in RGBA[i:i+3]], RGBA[i+3])),
         tags={338: (3, [1])})
    tiff('gray16.tiff', pixels=struct.pack('<'+'H'*(W*H), *[(x*1901+y*97) % 65536 for y in range(H) for x in range(W)]),
         channels=1, bits=16, photo=1)
    tiff('bigtiff-le.tiff', big=True)
    tiff('bigtiff-be.tiff', big=True, endian='>')
    tiff('planar.tiff', planar=True)
    tiff('tiled.tiff', tiled=True)
    tiff('deflate.tiff', compression=8)
    tiff('packbits.tiff', compression=32773)
    tiff('palette.tiff', channels=1, photo=3, pixels=bytes(x+3*y for y in range(H) for x in range(W)),
         tags={320: (3, [i*257 for i in range(256)] + [(255-i)*257 for i in range(256)] + [(i*5 % 256)*257 for i in range(256)])})
    tiff('rotated.tiff', tags={274: (3, [6]), 270: (2, b'Recast orientation fixture\0'), 315: (2, b'Recast test artist\0'),
         700: (1, b'<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/" dc:format="image/tiff"/></rdf:RDF></x:xmpmeta>'),
         33723: (7, b'\x1c\x02\x05\0\x06Recast')})
    tiff('rgba16.tiff', channels=4, bits=16,
         pixels=struct.pack('<'+'H'*(W*H*4), *[c for y in range(H) for x in range(W)
                           for c in (x*1901, y*3101, (x+y)*997, [0, 32768, 65535][x % 3])]))
    png = (ROOT / 'linear-rgb.png').read_bytes()
    pos = 8
    while pos < len(png):
        size, tag = struct.unpack('>I4s', png[pos:pos+8])
        if tag == b'iCCP':
            profile = zlib.decompress(png[pos+8:pos+8+size].split(b'\0', 1)[1][1:])
            break
        pos += 12+size
    else:
        raise RuntimeError('Missing synthetic ICC profile')
    tiff('profiled.tiff', tags={34675: (7, profile)})
    # A real second directory sharing the pixel payload, with a terminating link.
    multi = bytearray((ROOT / 'rgb-le.tiff').read_bytes())
    count = struct.unpack_from('<H', multi, 8)[0]
    end = 8+2+count*12
    second = len(multi)
    multi.extend(multi[8:end+4])
    struct.pack_into('<I', multi, end, second)
    (ROOT / 'multipage.tiff').write_bytes(multi)
