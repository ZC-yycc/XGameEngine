import json, sys

def load(path):
    d = json.load(open(path, encoding="utf-8"))
    p = d["payload"] if "payload" in d else d
    out = {}
    for e in p["entities"]:
        comps = {}
        for c in e["components"]:
            comps[c["component"]] = {n: v for n, v in c["fields"]}
        out[str(e["id"])] = comps
    return out

def vec(comps, comp, field):
    v = comps[comp][field]["value"]
    return tuple(v) if isinstance(v, list) else v

frames = [int(a) for a in sys.argv[1:]] or [1,2,5,10,30,60,100,150]

# entity 5 rotation (comp 257 rotation), entities 6..9 transform position/rotation, entity 14 animator
print("frame | spinner_rot_y(deg) | bouncer_y | crateA_y | crateB_y | crateC_y | fox_anim_base_pos | fox_pos")
for n in frames:
    O = load(f"tmp/old-{n}.json"); X = load(f"tmp/xgc-{n}.json")
    def quat_angle_y(comps):
        q = vec(comps, 257, "rotation")  # (x,y,z,w)
        import math
        return round(2.0*math.degrees(math.atan2(q[1], q[3])), 4) if abs(q[3])>1e-9 else None
    so = quat_angle_y(O["5"]); sx = quat_angle_y(X["5"])
    def y(comps, eid): return round(vec(comps[eid], 257, "position")[1], 6)
    fo = vec(O["14"], 1281, "base_position") if "base_position" in O["14"].get(1281, {}) else None
    fx = vec(X["14"], 1281, "base_position") if "base_position" in X["14"].get(1281, {}) else None
    print(f"{n:>3} | {so} vs {sx} | {y(O,'6')} vs {y(X,'6')} | {y(O,'7')} vs {y(X,'7')} | {y(O,'8')} vs {y(X,'8')} | {y(O,'9')} vs {y(X,'9')} | {fo} vs {fx} | {round(vec(O['14'],257,'position')[0],6)} vs {round(vec(X['14'],257,'position')[0],6)}")
