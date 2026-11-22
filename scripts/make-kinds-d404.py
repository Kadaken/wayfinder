#!/usr/bin/env python3
"""D-404: makes synthetic sample files for each unreadable-file kind into DIR.
Usage: make-kinds-d404.py DIR   (needs ffmpeg with libx265, python3 stdlib only)."""
import os, random, struct, subprocess, sys

out = os.path.abspath(sys.argv[1])
os.makedirs(out, exist_ok=True)

def ff(*args):
    subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-y", *args], check=True, cwd=out)

def p(name):
    return os.path.join(out, name)

def box(kind, payload):
    return struct.pack(">I4s", 8 + len(payload), kind) + payload

def fullbox(kind, version, flags, payload):
    return box(kind, struct.pack(">I", (version << 24) | flags) + payload)

def annexb_nals(data):
    nals, i, starts = [], 0, []
    while True:
        j = data.find(b"\x00\x00\x01", i)
        if j < 0:
            break
        starts.append(j + 3)
        i = j + 3
    for k, s in enumerate(starts):
        e = starts[k + 1] - 3 if k + 1 < len(starts) else len(data)
        nal = data[s:e].rstrip(b"\x00") if k + 1 < len(starts) else data[s:e]
        nals.append(nal)
    return nals

def make_heic(path, cols=2, rows=2, tile=128):
    tiles, params = [], {}
    for n in range(cols * rows):
        raw = p(f"tile{n}.hevc")
        ff("-f", "lavfi", "-i", f"testsrc2=s={tile}x{tile}:d=1,hue=h={n * 60}", "-frames:v", "1",
           "-c:v", "libx265", "-x265-params", "keyint=1:repeat-headers=1:log-level=none",
           "-pix_fmt", "yuv420p", "-f", "hevc", raw)
        slices = b""
        for nal in annexb_nals(open(raw, "rb").read()):
            t = (nal[0] >> 1) & 0x3F
            if t in (32, 33, 34):
                params[t] = nal
            elif t < 32:
                slices += struct.pack(">I", len(nal)) + nal
        tiles.append(slices)
        os.remove(raw)
    sps = params[33]
    # hvcC: general profile fields copied from the SPS profile_tier_level (bytes 3..15).
    ptl = sps[3:15]
    arrays = b""
    for t in (32, 33, 34):
        arrays += struct.pack(">BH", 0x80 | t, 1) + struct.pack(">H", len(params[t])) + params[t]
    hvcc = box(b"hvcC", bytes([1]) + ptl + struct.pack(">H", 0xF000)
               + bytes([0xFC, 0xFD, 0xF8, 0xF8]) + struct.pack(">H", 0) + bytes([0x0F, 3]) + arrays)
    W, H = cols * tile, rows * tile
    grid_id, first_tile = 100, 1
    grid_data = struct.pack(">BBBBHH", 0, 0, rows - 1, cols - 1, W, H)
    ids = [grid_id] + [first_tile + n for n in range(len(tiles))]
    datas = [grid_data] + tiles

    def build(offsets):
        ftyp = box(b"ftyp", b"heic" + struct.pack(">I", 0) + b"mif1heic")
        hdlr = fullbox(b"hdlr", 0, 0, struct.pack(">I4s12s", 0, b"pict", b"") + b"\x00")
        pitm = fullbox(b"pitm", 0, 0, struct.pack(">H", grid_id))
        infe = lambda i, typ, hidden: fullbox(b"infe", 2, 1 if hidden else 0, struct.pack(">HH4s", i, 0, typ) + b"\x00")
        iinf = fullbox(b"iinf", 0, 0, struct.pack(">H", len(ids)) + infe(grid_id, b"grid", False)
                       + b"".join(infe(i, b"hvc1", True) for i in ids[1:]))
        iloc_items = b"".join(struct.pack(">HHHII", i, 0, 1, o, len(d)) for i, o, d in zip(ids, offsets, datas))
        iloc = fullbox(b"iloc", 0, 0, bytes([0x44, 0x00]) + struct.pack(">H", len(ids)) + iloc_items)
        ispe = lambda w, h: fullbox(b"ispe", 0, 0, struct.pack(">II", w, h))
        ipco = box(b"ipco", ispe(W, H) + hvcc + ispe(tile, tile))
        assoc = struct.pack(">HB", grid_id, 1) + bytes([1])
        for i in ids[1:]:
            assoc += struct.pack(">HB", i, 2) + bytes([2, 3])
        ipma = fullbox(b"ipma", 0, 0, struct.pack(">I", len(ids)) + assoc)
        iprp = box(b"iprp", ipco + ipma)
        dimg = box(b"dimg", struct.pack(">HH", grid_id, len(ids) - 1) + b"".join(struct.pack(">H", i) for i in ids[1:]))
        iref = fullbox(b"iref", 0, 0, dimg)
        meta = fullbox(b"meta", 0, 0, hdlr + pitm + iinf + iloc + iprp + iref)
        return ftyp, meta

    ftyp, meta = build([0] * len(ids))
    base = len(ftyp) + len(meta) + 8
    offsets, o = [], base
    for d in datas:
        offsets.append(o)
        o += len(d)
    ftyp, meta = build(offsets)
    open(path, "wb").write(ftyp + meta + box(b"mdat", b"".join(datas)))

def cut(src, dst, frac):
    b = open(src, "rb").read()
    open(dst, "wb").write(b[: int(len(b) * frac)])

# A: HEIC tile grid, named .heic and .jpg
make_heic(p("a_grid.heic"))
open(p("a_grid_named_jpg.jpg"), "wb").write(open(p("a_grid.heic"), "rb").read())

# real pictures, noisy so they do not compress to nothing
ff("-f", "lavfi", "-i", "testsrc2=s=320x240:d=1", "-frames:v", "1", "full.png")
ff("-f", "lavfi", "-i", "testsrc2=s=320x240:d=1", "-frames:v", "1", "-q:v", "2", "full.jpg")
ff("-f", "lavfi", "-i", "testsrc2=s=160x120:d=1,format=rgb8", "-frames:v", "1", "full.gif")
# B: truncated
cut(p("full.jpg"), p("b_trunc.jpg"), 0.6)
cut(p("full.png"), p("b_trunc.png"), 0.6)
cut(p("full.gif"), p("b_trunc.gif"), 0.6)
# C: pictures under wrong extension or none
for src, dst in (("full.gif", "c_gif.js"), ("full.jpg", "c_jpg.htm"), ("full.png", "c_png"), ("full.jpg", "c_jpg.css")):
    open(p(dst), "wb").write(open(p(src), "rb").read())
cut(p("full.jpg"), p("c_trunc_jpg.xml"), 0.6)
# D: avif and a picture-type .MOV (a JPEG inside a .MOV)
ff("-i", "full.png", "-frames:v", "1", "-c:v", "libaom-av1", "-cpu-used", "8", "-still-picture", "1", "d_pic.avif")
open(p("d_pic.MOV"), "wb").write(open(p("full.jpg"), "rb").read())
# E: mp4 with no moov (cut the moov box off the end, keep ftyp + mdat)
ff("-f", "lavfi", "-i", "testsrc2=s=160x120:d=2:r=15", "-c:v", "libx264", "-pix_fmt", "yuv420p", "good.mp4")
b = open(p("good.mp4"), "rb").read()
pos, end_mdat = 0, 0
while pos + 8 <= len(b):
    size, kind = struct.unpack(">I4s", b[pos:pos + 8])
    if kind == b"moov":
        break
    end_mdat = pos + size
    pos += size
open(p("e_nomoov.mp4"), "wb").write(b[:end_mdat])  # recording cut off before the index was written
# F: MPEG-PS video with wrong or no extension
ff("-f", "lavfi", "-i", "testsrc2=s=160x120:d=7:r=15", "-c:v", "mpeg2video", "-b:v", "400k", "-f", "mpeg", "f_good.mpg")
for dst in ("f_vid", "f_vid.htm", "f_vid.js"):
    open(p(dst), "wb").write(open(p("f_good.mpg"), "rb").read())
# G: .mts with no TS header; H: RIFF/AVI header, no streams; I: all zero; J: random bytes named .jpg
rnd = random.Random(404)
open(p("g_junk.mts"), "wb").write(bytes(rnd.getrandbits(8) for _ in range(40000)))
open(p("h_nostreams.avi"), "wb").write(b"RIFF" + struct.pack("<I", 4 + 12) + b"AVI " + b"LIST" + struct.pack("<I", 4) + b"hdrl")
open(p("i_zero.jpg"), "wb").write(bytes(60000))
open(p("j_random.jpg"), "wb").write(bytes(rnd.getrandbits(8) for _ in range(30000)))
for helper in ("full.png", "full.jpg", "full.gif", "good.mp4", "f_good.mpg"):
    os.remove(p(helper))
