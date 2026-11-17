#!/usr/bin/env python3
"""Generates a tiny multi-page test PDF (no external deps) for testing Glossa."""
import sys, zlib

def make_pdf(path, title=None, pages=6, w=595, h=842):
    title = title or path.rsplit("/", 1)[-1]
    objs = []
    # 1: catalog, 2: pages tree, then per page: 3 objects
    content_per_page = []
    for i in range(pages):
        c = f"BT /F1 36 Tf 72 {h-120} Td ({title}) Tj ET\n"
        c += f"BT /F1 20 Tf 72 {h-180} Td (Page {i+1} of {pages}) Tj ET\n"
        # a paragraph block so text visually fills the page
        lines = ["A man may imagine things that are false, but he can only",
                 "understand things that are true, for if the things be false,",
                 "the apprehension of them is... something else."]
        for ln_i, ln in enumerate(lines[:2 + (i % 2)]):
            c += f"BT /F1 12 Tf 72 {h-260 - 18*ln_i} Td ({ln}) Tj ET\n"
        rects = [("yellow", 400, h-400, 500, h-340)]
        c += "0.99 0.91 0.51 rg %g %g %g %g re f\n0 0 0 RG\n" % (400, h-400, 100, 60)
        content_per_page.append(c)

    # build objects
    def page_obj(i):
        content_stream = content_per_page[i].encode()
        return {
            "page": None,  # filled later
        }

    # We'll construct object list manually with known numbering:
    # 1 catalog, 2 pages, 3 font, then per page: page obj + content obj
    n_pages = pages
    first_page_obj = 4
    objs = {}
    objs[1] = "<< /Type /Catalog /Pages 2 0 R >>"
    kids = " ".join(f"{first_page_obj + 2*i} 0 R" for i in range(n_pages))
    objs[2] = f"<< /Type /Pages /Count {n_pages} /Kids [{kids}] >>"
    objs[3] = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"
    for i in range(n_pages):
        po = first_page_obj + 2 * i
        co = po + 1
        objs[po] = (f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] "
                    f"/Resources << /Font << /F1 3 0 R >> >> /Contents {co} 0 R >>")
        data = zlib.compress(content_per_page[i].encode())
        objs[co] = ("STREAM", data, f"<< /Length {len(data)} /Filter /FlateDecode >>")

    out = bytearray(b"%PDF-1.4\n")
    offsets = {}
    for num in sorted(objs):
        offsets[num] = len(out)
        if isinstance(objs[num], tuple):
            _, data, meta = objs[num]
            out += f"{num} 0 obj\n{meta}\nstream\n".encode()
            out += data
            out += b"\nendstream\nendobj\n"
        else:
            out += f"{num} 0 obj\n{objs[num]}\nendobj\n".encode()
    xref_at = len(out)
    maxnum = max(objs)
    out += f"xref\n0 {maxnum + 1}\n".encode()
    out += b"0000000000 65535 f \n"
    for num in range(1, maxnum + 1):
        if num in offsets:
            out += f"{offsets[num]:010d} 00000 n \n".encode()
        else:
            out += b"0000000000 00000 f \n"
    out += (f"trailer\n<< /Size {maxnum + 1} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n").encode()
    with open(path, "wb") as f:
        f.write(out)
    print(f"wrote {path} ({pages} pages)")

if __name__ == "__main__":
    args = sys.argv[1:]
    make_pdf(args[0], title=args[1] if len(args) > 1 else None,
             pages=int(args[2]) if len(args) > 2 else 6)
