import json, sys

NUMERIC = {"vec3","vec4","quat","f64","f32","i64","u64","i32","u32"}

def load(path):
    d = json.load(open(path, encoding="utf-8"))
    p = d["payload"] if "payload" in d else d
    out = {}
    for e in p["entities"]:
        comps = {}
        for c in e["components"]:
            fields = {}
            for name, val in c["fields"]:
                fields[name] = val
            comps[c["component"]] = fields
        out[str(e["id"])] = comps
    return out

def num(val):
    kind = val.get("kind")
    v = val.get("value")
    return kind, v

def diff_fields(a, b):
    """returns list of (field, max_abs_delta) for numeric fields with delta > 1e-7"""
    diffs = []
    for name, va in a.items():
        if name not in b:
            continue
        ka, va_ = num(va)
        kb, vb_ = num(b[name])
        if ka != kb or ka not in NUMERIC:
            continue
        if ka == "quat":
            # q and -q are the same rotation.
            if sum(x*y for x, y in zip(va_, vb_)) < 0:
                vb_ = [-x for x in vb_]
            d = max(abs(x-y) for x, y in zip(va_, vb_))
        elif isinstance(va_, list):
            d = max(abs(x-y) for x, y in zip(va_, vb_))
        else:
            d = abs(va_ - vb_)
        if d > 1e-7:
            diffs.append((name, d))
    return diffs

def main(old, new, tol):
    A, B = load(old), load(new)
    ids = sorted(set(A) & set(B), key=lambda s: int(s))
    missing = (set(A) ^ set(B))
    print(f"# {old} vs {new}: entities {len(A)}/{len(B)}, id mismatch {sorted(missing)[:5]}")
    worst = []
    for eid in ids:
        for comp in sorted(set(A[eid]) & set(B[eid])):
            fs = diff_fields(A[eid][comp], B[eid][comp])
            if not fs:
                continue
            mx = max(d for _, d in fs)
            fields = ",".join(f"{n}:{d:.3g}" for n, d in sorted(fs, key=lambda p: -p[1])[:4])
            worst.append((mx, eid, comp, fields))
    worst.sort(reverse=True)
    for mx, eid, comp, fields in worst:
        mark = "BIG" if mx > tol else "small"
        print(f"{mark} entity {eid:>2} comp {comp}: max {mx:.6g} ({fields})")

if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], float(sys.argv[3]) if len(sys.argv) > 3 else 1e-3)
