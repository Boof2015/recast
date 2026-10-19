"""Synthetic Opus fixture using libopus directly, without FFmpeg's Ogg muxer.

Run with the pinned Opus install directory as the only argument. CI consumes
the checked-in fixture. Uses 10 ms packets, continued comments, header gain,
an informational 44.1 kHz original-rate field, and an odd final frame count.
"""
import math
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent
REFERENCE = r'''
#include <opus.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
static void put32(unsigned n) {
    for (int i=0;i<4;i++) putchar((n>>(8*i))&255);
}
static unsigned get32(void) {
    unsigned n=0;
    for (int i=0;i<4;i++) { int b=getchar(); if(b==EOF) exit(i?2:0); n|=(unsigned)b<<(8*i); }
    return n;
}
int main(int argc, char **argv) {
    int error;
    if (argc<2) return 1;
    if (!strcmp(argv[1],"encode")) {
        OpusEncoder *encoder=opus_encoder_create(48000,2,OPUS_APPLICATION_AUDIO,&error);
        if (!encoder || error) return 2;
        if (opus_encoder_ctl(encoder,OPUS_SET_BITRATE(192000))<0) return 3;
        int skip;
        if (opus_encoder_ctl(encoder,OPUS_GET_LOOKAHEAD(&skip))<0) return 4;
        fprintf(stderr,"%d\n",skip);
        float samples[960]; unsigned char packet[4000];
        while (fread(samples,sizeof(float),960,stdin)==960) {
            int n=opus_encode_float(encoder,samples,480,packet,sizeof(packet));
            if(n<0) return 5;
            put32(n); fwrite(packet,1,n,stdout);
        }
        opus_encoder_destroy(encoder);
    } else {
        OpusDecoder *decoder=opus_decoder_create(48000,2,&error);
        if (!decoder || error) return 6;
        if (argc>2 && opus_decoder_ctl(decoder,OPUS_SET_GAIN(atoi(argv[2])))<0) return 7;
        for (;;) {
            unsigned n=get32(); unsigned char packet[65536]; float pcm[11520];
            if(n>sizeof(packet) || fread(packet,1,n,stdin)!=n) return 8;
            int frames=opus_decode_float(decoder,packet,n,pcm,5760,0);
            if(frames<0) return 9;
            fwrite(pcm,sizeof(float),frames*2,stdout);
        }
    }
    return 0;
}
'''


def build_reference(stage, work):
    source = work / 'reference.c'
    source.write_text(REFERENCE)
    executable = work / 'reference'
    subprocess.run(['cc', '-O2', '-I', str(stage / 'include/opus'), str(source),
                    str(stage / 'lib/libopus.a'), '-lm', '-o', str(executable)], check=True)
    return executable


def crc(data):
    value = 0
    for byte in data:
        value ^= byte << 24
        for _ in range(8):
            value = ((value << 1) ^ (0x04c11db7 if value & 0x80000000 else 0)) & 0xffffffff
    return value


def page(sequence, flags, granule, data, lacing=None):
    if lacing is None:
        lacing = bytes([255] * (len(data) // 255) + [len(data) % 255])
    header = bytearray(b'OggS' + bytes([0, flags]) + struct.pack('<QIII', granule, 0x52454341, sequence, 0) + bytes([len(lacing)]))
    result = header + lacing + data
    result[22:26] = struct.pack('<I', crc(result))
    return result


def unpack_packets(data):
    packets = []
    while data:
        size, = struct.unpack('<I', data[:4])
        packets.append(data[4:4+size])
        data = data[4+size:]
    return packets


def main():
    with tempfile.TemporaryDirectory(prefix='recast-opus-fixture-') as temporary:
        encoder = build_reference(Path(sys.argv[1]), Path(temporary))
        frames = 48117
        # Extra silent input flushes the encoder lookahead; the EOS granule
        # trims this padding. Source is stereo 431/997 Hz at amplitude 0.2.
        pcm = b''.join(struct.pack('<ff', *(0.2 * math.sin(i*f*math.tau/48000) if i < frames else 0.0 for f in [431, 997])) for i in range(49440))
        encoded = subprocess.run([encoder, 'encode'], input=pcm, capture_output=True, check=True)
        preskip = int(encoded.stderr)
        packets = unpack_packets(encoded.stdout)[:math.ceil((frames + preskip) / 480)]
        head = b'OpusHead' + bytes([1, 2]) + struct.pack('<HIhB', preskip, 44100, 3*256, 0)
        result = page(0, 2, 0, head)
        vendor = b'Recast direct libopus 1.6.1 fixture' + b' ' * 66000
        comment = b'TITLE=Independent tone'
        tags = b'OpusTags' + struct.pack('<I', len(vendor)) + vendor + struct.pack('<II', 1, len(comment)) + comment
        result += page(1, 0, 0xffffffffffffffff, tags[:65025], bytes([255])*255)
        result += page(2, 1, 0, tags[65025:])
        for i, packet in enumerate(packets):
            last = i == len(packets)-1
            result += page(i+3, 4 if last else 0, frames+preskip if last else (i+1)*480, packet)
        (ROOT / 'reference-opus.opus').write_bytes(result)


if __name__ == '__main__':
    main()
