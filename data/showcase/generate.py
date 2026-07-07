# Regenerate the showcase dataset (deterministic — same seed, same bytes).
#   python3 data/showcase/generate.py
# The showcase project (data/projects/showcase/) mounts these files and composes
# every view the twin can draw into one dashboard.
import json
import math
import random
import os

random.seed(42)
HERE = os.path.dirname(os.path.abspath(__file__))

# ---- work orders: dates, spans, enums, numbers, geo, prose — every detector fires
STATUSES = ["open", "in progress", "blocked", "done", "cancelled"]
CREWS = ["alpha", "bravo", "charlie", "delta"]
UNITS = ["pump P-101", "compressor K-201", "separator V-301", "turbine T-401",
         "heat exchanger E-501", "valve manifold M-601", "generator G-701"]
BASE = 1735689600000  # 2025-01-01 UTC (ms)
DAY = 86400000

rows = []
for i in range(1500):
    day = int(random.triangular(0, 240, 200))          # activity ramps up
    hour = int(random.triangular(5, 22, 9))            # day-shift heavy
    created = BASE + day * DAY + hour * 3600000 + random.randint(0, 3500000)
    dur_days = random.triangular(0.5, 21, 3)
    started = created + int(random.uniform(0.2, 3) * DAY)
    finished = started + int(dur_days * DAY)
    unit = random.choice(UNITS)
    vib = abs(random.gauss(2.4, 0.9))
    if random.random() < 0.01:
        vib += random.uniform(8, 14)                   # the spikes a chart must keep
    crew = random.choice(CREWS)
    if i % 4 == 0:
        report = (f"## Findings\n\nInspection of **{unit}** by crew {crew}.\n\n"
                  f"- vibration at `{vib:.1f} mm/s`\n"
                  f"- bearing temperature within limits\n"
                  f"- housing corrosion grade {random.randint(1, 4)}\n\n"
                  f"Recommend follow-up in {random.randint(2, 12)} weeks.")
    else:
        report = (f"Routine pass on {unit}: no anomalies found. Torque values logged "
                  f"and archived, lubrication per schedule, next window unchanged. "
                  f"Crew {crew} closed out after {dur_days:.1f} days on site.")
    rows.append({
        "id": i,
        "title": f"WO-{1000 + i}: {random.choice(['inspect', 'overhaul', 'calibrate', 'replace seals on', 'flush'])} {unit}",
        "status": random.choices(STATUSES, weights=[4, 3, 1, 7, 1])[0],
        "crew": crew,
        "created": created,
        "started": started,
        "finished": finished,
        "hours": round(random.triangular(1, 30, 6), 1),
        "cost": round(abs(random.gauss(1800, 1100)), 0),
        "vibration": round(vib, 2),
        "lat": round(56.9 + random.random() * 1.6, 5),
        "lon": round(3.2 + random.random() * 1.1, 5),
        "report": report,
    })
with open(os.path.join(HERE, "workorders.jsonl"), "w") as f:
    for r in rows:
        f.write(json.dumps(r) + "\n")

# ---- well paths: three deviated wells — build, turn, land out — for the 3d view
with open(os.path.join(HERE, "wellpaths.csv"), "w") as f:
    f.write("well,md,east,north,tvd\n")
    for w, (heading, kick, flip) in enumerate([(0.4, 900, 1), (2.4, 1150, -1), (4.4, 1000, 1)]):
        east = north = tvd = 0.0
        inc = 0.0
        for step in range(220):
            md = step * 15.0
            if md > kick:
                inc = min(math.radians(88), inc + math.radians(0.55))
            azi = heading + flip * min(md / 4000.0, 0.9)
            east += 15.0 * math.sin(inc) * math.sin(azi)
            north += 15.0 * math.sin(inc) * math.cos(azi)
            tvd += 15.0 * math.cos(inc)
            f.write(f"well-{chr(65 + w)},{md:.0f},{east:.1f},{north:.1f},{tvd:.1f}\n")

print(f"wrote {len(rows)} work orders and 3 well paths under {HERE}")
