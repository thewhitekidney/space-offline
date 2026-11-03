#!/usr/bin/env python3
"""Check that a warp position-vs-time curve closes against our flight time.

`destiny::warp_flight_seconds` implements CCP's warp model as three phases.
Position against time falls out of the same three constants, and if the curve
evaluated at the full flight duration lands on the distance the function
budgeted for, a server-side warp integrator needs no new facts at all -- it is
the client's own model read the other way round. See
`docs/port-tracker.md` §37.4, which this script is the falsification for.

Prints one line per case; `err` must be zero and `mono` must be True.
Usage: python3 -I tools/re/warp_curve.py
"""

import math
AU = 149_597_870_700.0

def phases(d, wf):
    k_a = wf * 0.001
    k_d = min(wf / 3000.0, 2.0)
    v_c = min((d + 1.0) * k_a * k_d / (k_a + k_d), k_a * AU)
    t_a = math.log(v_c / k_a) / k_a
    t_d = math.log(v_c / k_d) / k_d
    t_c = max(d / v_c - 1.0 / k_a - 1.0 / k_d + 1.0 / v_c, 0.0)
    return k_a, k_d, v_c, max(t_a, 0.0), t_c, max(t_d, 0.0)

def travelled(d, wf, t):
    """Metres covered t seconds into the warp phase (align excluded)."""
    k_a, k_d, v_c, t_a, t_c, t_d = phases(d, wf)
    s_a = v_c / k_a - 1.0
    if t <= 0.0:
        return 0.0
    if t < t_a:
        return math.exp(k_a * t) - 1.0
    if t < t_a + t_c:
        return s_a + v_c * (t - t_a)
    tp = t - t_a - t_c
    if tp >= t_d:
        return d - 1.0
    # Remaining distance decays exponentially from v_c/k_d down to 1 m.
    return d - 1.0 - ((v_c / k_d) * math.exp(-k_d * tp) - 1.0)

for d, wf in [(581.0, 4000), (5*AU, 4000), (0.4*AU, 1350), (100*AU, 6000), (1e6, 3000)]:
    k_a, k_d, v_c, t_a, t_c, t_d = phases(d, wf)
    flight = t_a + t_c + t_d
    end = travelled(d, wf, flight)
    mono = all(travelled(d, wf, flight*i/200) <= travelled(d, wf, flight*(i+1)/200) + 1e-6
               for i in range(200))
    print(f"d={d:>18.1f} wf={wf:>5} v_c={v_c:>15.1f} t={flight:>8.2f}s "
          f"end={end:>18.3f} want={d-1.0:>18.3f} err={end-(d-1.0):+.3e} mono={mono}")
