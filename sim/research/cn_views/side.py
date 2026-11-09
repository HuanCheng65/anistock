import json, sys, statistics as S, math, sqlite3
R='/home/starrydream/anisotck/'
old=json.load(open(R+'data/derived/markets.json'))['series']; new=json.load(open(R+'data/fast/cnv/markets.json'))['series']
db=sqlite3.connect(f'file:{R}data/anistock.db?mode=ro',uri=True)
title={int(i):(json.loads(d).get('title') or {}).get('native') for i,d in db.execute("select external_id,data from snapshots where source='anilist' and kind='media'")}
days=[d for d in sorted({d for s in new.values() for d in s}) if '2026-10-07'<=d<='2026-11-05']
rows=[]
for aid in set(old)|set(new):
    o=[old.get(aid,{}).get(d,{}).get('cn') for d in days]; n=[new.get(aid,{}).get(d,{}).get('cn') for d in days]
    j=[(old.get(aid,{}).get(d,{}) or new.get(aid,{}).get(d,{})).get('jp') for d in days]
    oo=[x for x in o if x]; nn=[x for x in n if x]; jj=[x for x in j if x]
    rows.append(dict(aid=aid,t=title.get(int(aid)),o=S.mean(oo) if oo else None,n=S.mean(nn) if nn else None,j=S.mean(jj) if jj else None,
        o_last=S.mean(oo[-7:])/S.mean(oo[:7]) if len(oo)>=21 else None, n_last=S.mean(nn[-7:])/S.mean(nn[:7]) if len(nn)>=21 else None))
both=[r for r in rows if r['o'] and r['n']]
print('shows: old cn',sum(1 for r in rows if r['o']),' new cn',sum(1 for r in rows if r['n']),' both',len(both))
print('only new:',[r['t'] for r in rows if r['n'] and not r['o']][:20])
print('only old:',[r['t'] for r in rows if r['o'] and not r['n']][:20])
L=lambda k,rs:[math.log(r[k]) for r in rs]
bj=[r for r in both if r['j']]
print('level across shows (Oct 7–Nov 5 mean): corr with jp  old %.2f  new %.2f   old~new %.2f'%(S.correlation(L('o',bj),L('j',bj)),S.correlation(L('n',bj),L('j',bj)),S.correlation(L('o',both),L('n',both))))
k=S.median(r['n']/r['o'] for r in both)
print('median new/old %.1f; most raised / lowered relative to that (ratio / median):'%k)
srt=sorted(both,key=lambda r:r['n']/r['o'])
for r in srt[:8]+[None]+srt[-8:]:
    if r is None: print('  ...'); continue
    print(f"  {r['n']/r['o']/k:6.2f}  old {r['o']:8.1f}  new {r['n']:8.1f}  jp {r['j'] or 0:7.1f}  {r['t']}")
sh=[r for r in both if r['o_last'] and r['n_last']]
print('last week / first week of the window, median: old %.2f new %.2f'%(S.median(r['o_last'] for r in sh),S.median(r['n_last'] for r in sh)))
