# Research: per-show weekly panels, Bangumi weekly archive stocks + JP (Annict) + AniList episode schedule.
import json, sqlite3, pickle, sys
from datetime import date, datetime, timedelta
from collections import defaultdict
R='/home/starrydream/anisotck/'
b=sqlite3.connect(f'file:{R}data/derived/heat_backfill.db?mode=ro',uri=True)
c=sqlite3.connect(f'file:{R}data/fast/collector_b.db?mode=ro',uri=True)
m=json.load(open(R+'sim/matches.json'))
a2b={int(k):int(v['bgm']) for k,v in m['matches'].items() if v.get('bgm')}
for k,v in m.get('manual',{}).items():
    if isinstance(v,dict) and v.get('bgm'): a2b[int(k)]=int(v['bgm'])
for k,v in json.load(open(R+'data/derived/archive_matches.json')).items(): a2b.setdefault(int(k),int(v))
snaps=[r[0] for r in b.execute("select at from bgm_anime_dumps order by at")]
# keep one dump per week (drop the 2026-03-18 re-runs)
wk=[]; 
for s in snaps:
    if not wk or (datetime.fromisoformat(s[:19])-datetime.fromisoformat(wk[-1][:19])).days>=6: wk.append(s)
stock=defaultdict(dict)
for sid,at,wish,done,doing,hold,drop in b.execute("select subject_id, at, wish, done, doing, on_hold, dropped from bgm_anime_weekly"):
    stock[sid][at]=(wish or 0,done or 0,doing or 0,hold or 0,drop or 0)
heat=defaultdict(dict)
for w,d,us,cn,jp in b.execute("select work_id, day, us, cn, jp from heat_history"): heat[w][d]=(us or 0,cn or 0,jp or 0)
eps=defaultdict(dict)
for mid,d,e in c.execute("select media_id, date, episode from anilist_trends where episode is not null"): eps[mid][d]=e
out={}
for w,sid in a2b.items():
    if w not in eps or sid not in stock: continue
    st=stock[sid]
    out[w]=dict(sid=sid, eps=eps[w], heat=heat.get(w,{}), stock={a:st.get(a) for a in wk})
pickle.dump(dict(weeks=wk, shows=out), open(sys.argv[1],'wb'))
print(len(out),'shows', len(wk),'weeks')
