#!/usr/bin/env python3
"""Synthetic GIF containers/LZW and WebP muxing independent of ImageMagick.
WebP frame payloads reuse existing still fixtures; GIF reference canvases are
composed here and compared with the worker and an independent browser decoder.
"""
from pathlib import Path
import json
import struct

ROOT = Path(__file__).resolve().parent
u16 = lambda n: struct.pack('<H', n)
u24 = lambda n: n.to_bytes(3, 'little')
u32 = lambda n: struct.pack('<I', n)
PALETTE = bytes([0,0,0, 255,0,0, 0,255,0, 0,0,255])

def subblocks(data):
    return b''.join(bytes([len(data[i:i+255])]) + data[i:i+255] for i in range(0,len(data),255)) + b'\0'

def lzw(pixels):
    # Clear before every literal: code size remains three bits throughout.
    codes = [code for pixel in pixels for code in (4,pixel)] + [5]
    value = sum(code << (3*i) for i,code in enumerate(codes))
    return bytes([2]) + subblocks(value.to_bytes((3*len(codes)+7)//8,'little'))

def gif(name, width, height, frames, loops=1, interlaced=False, local=False):
    out = b'GIF89a' + u16(width) + u16(height) + bytes([0x81,0,0]) + PALETTE
    if loops != 1:
        out += b'\x21\xff\x0bNETSCAPE2.0\x03\x01' + u16(0 if loops == 0 else loops-1) + b'\0'
    canvas = [[0,0,0,0] for _ in range(width*height)]
    expected = []
    for x,y,w,h,pixels,delay,disposal in frames:
        before = [p[:] for p in canvas]
        out += b'\x21\xf9\x04' + bytes([(disposal<<2)|1]) + u16(delay//10) + b'\0\0'
        flags = (0x40 if interlaced else 0) | (0x81 if local else 0)
        out += b'\x2c' + u16(x)+u16(y)+u16(w)+u16(h)+bytes([flags])
        if local: out += PALETTE
        ordered = pixels
        if interlaced:
            rows = list(range(0,h,8))+list(range(4,h,8))+list(range(2,h,4))+list(range(1,h,2))
            ordered = [pixels[row*w+column] for row in rows for column in range(w)]
        out += lzw(ordered)
        for row in range(h):
            for col in range(w):
                index = pixels[row*w+col]
                if index: canvas[(y+row)*width+x+col] = list(PALETTE[index*3:index*3+3])+[255]
        expected.append([v for p in canvas for v in p])
        if disposal == 2:
            for row in range(h):
                for col in range(w): canvas[(y+row)*width+x+col] = [0,0,0,0]
        elif disposal == 3: canvas = before
    (ROOT/name).write_bytes(out+b';')
    return expected

pixels = [(x+y)%4 for y in range(20) for x in range(32)]
gif('still.gif',32,20,[(0,0,32,20,pixels,0,0)])
gif('interlaced.gif',32,20,[(0,0,32,20,pixels,0,0)],interlaced=True,local=True)
frames = [(0,0,8,6,[1]*48,40,1), (2,2,2,2,[2]*4,80,3), (4,0,2,2,[3]*4,120,2), (0,0,4,4,[0,2,0,2]*4,0,1)]
expected = gif('disposal.gif',8,6,frames,loops=3)
gif('forever.gif',8,6,frames,loops=0)
gif('once.gif',8,6,frames)
gif('max-loops.gif',8,6,frames,loops=65536)
gif('identical.gif',8,6,[(0,0,8,6,[1]*48,d,1) for d in (20,30,40)],loops=2)
(ROOT/'disposal-frames.json').write_text(json.dumps(expected)+'\n')

def chunk(tag,payload): return tag + u32(len(payload)) + payload + b'\0'*(len(payload)%2)
def payload(name):
    data = (ROOT/name).read_bytes(); result = b''; offset = 12
    while offset < len(data):
        size = int.from_bytes(data[offset+4:offset+8],'little')
        if data[offset:offset+4] in (b'VP8 ',b'VP8L',b'ALPH'): result += data[offset:offset+8+size+size%2]
        offset += 8+size+size%2
    return result

def webp(name, delays, partial=False):
    width,height = (40,24) if partial else (32,20)
    body = chunk(b'VP8X',b'\x12\0\0\0'+u24(width-1)+u24(height-1))
    body += chunk(b'ANIM',b'\0\0\0\0'+u16(3))
    for i,delay in enumerate(delays):
        x,y = (2,2) if partial else (0,0)
        flags = [2,0,3,2][i%4] if partial else 2
        frame = u24(x//2)+u24(y//2)+u24(31)+u24(19)+u24(delay)+bytes([flags])+payload('rgba.webp' if i%2 else 'photo.webp')
        body += chunk(b'ANMF',frame)
    (ROOT/name).write_bytes(b'RIFF'+u32(len(body)+4)+b'WEBP'+body)
webp('millisecond.webp',[33,33,34,0])
webp('partial.webp',[40,80,120,0],partial=True)
webp('fast.webp',[1,5,10,20])
webp('long-delay.webp',[16_777_215,655360])

# Attach a synthetic linear ICC profile and EXIF/XMP to a complete animation,
# independently of ImageMagick's animated profile reader.
import zlib
png=(ROOT/'linear-rgb.png').read_bytes()
offset=8
while offset<len(png):
    size=int.from_bytes(png[offset:offset+4],'big')
    if png[offset+4:offset+8]==b'iCCP':
        compressed=png[offset+8:offset+8+size].split(b'\0',1)[1][1:]
        profile=zlib.decompress(compressed)
        break
    offset+=size+12
else: raise RuntimeError('Fixture is missing its ICC profile')
exif=b'II'+u16(42)+u32(8)+u16(1)+u16(274)+u16(3)+u32(1)+u16(6)+b'\0\0'+u32(0)
xmp=b'<x:xmpmeta xmlns:x="adobe:ns:meta/"><test>Recast animation</test></x:xmpmeta>'
body=bytearray((ROOT/'millisecond.webp').read_bytes()[12:])
body[8] |= 0x2c
body=bytes(body[:18])+chunk(b'ICCP',profile)+bytes(body[18:])+chunk(b'EXIF',exif)+chunk(b'XMP ',xmp)
(ROOT/'profiled-animation.webp').write_bytes(b'RIFF'+u32(len(body)+4)+b'WEBP'+body)
