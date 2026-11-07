"""History: mean weekly us log change by week since the premiere (w -> w+1), per season then averaged."""
import json, math, sqlite3, statistics as st
from collections import defaultdict
from datetime import date, timedelta
import sys; sys.path.insert(0, 'data/fast/an')
exec(open('data/fast/an/hist_gamma.py').read().split("print(len(shows), 'shows')")[0])
by = defaultdict(lambda: defaultdict(list))
for k, nw, att in shows:
    for w in range(0, 9):
        if w + 1 > nw - 4: continue
        a1, a2 = att[w], att[w + 1]
        if a1 and a2: by[w][k].append(math.log(a2 / a1))
for w in range(0, 9):
    per = [st.fmean(v) for v in by[w].values() if len(v) >= 5]
    if per:
        # odd/even years
        ev = [st.fmean(v) for k, v in by[w].items() if len(v) >= 5 and k[0] % 2 == 0]
        od = [st.fmean(v) for k, v in by[w].items() if len(v) >= 5 and k[0] % 2 == 1]
        print(f'week {w}->{w+1}: {st.fmean(per)/7:+.4f}/day (se {st.pstdev(per)/math.sqrt(len(per))/7:.4f}, {len(per)} seasons; even yrs {st.fmean(ev)/7:+.4f}, odd yrs {st.fmean(od)/7:+.4f}; summer seasons {st.fmean([st.fmean(v) for k, v in by[w].items() if len(v)>=5 and k[1]==2])/7:+.4f}, autumn {st.fmean([st.fmean(v) for k, v in by[w].items() if len(v)>=5 and k[1]==3])/7:+.4f})')
