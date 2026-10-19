#!/usr/bin/env python3
"""Утилита для design-matrix.jsonld (JSON-LD-lite матрица allspark).

Использование:
  python3 matrix.py validate    # проверить инварианты
  python3 matrix.py phases      # топосорт dependsOn → фазы
  python3 matrix.py stats       # статистика
  python3 matrix.py hub         # хабы по рёбрам
  python3 matrix.py turtle      # экспорт в N-Triples (для RDF-инструментов)
"""
import json, sys, collections
from collections import deque

PATH = '/root/prime-agent/.research/design-matrix.jsonld'

def load():
    rows = [json.loads(l) for l in open(PATH)]
    manifest = rows[0]
    feats = [r for r in rows[1:] if 'Feature' in r.get('@type','')]
    metas = [r for r in rows[1:] if 'Feature' not in r.get('@type','')]
    return manifest, feats, metas

def validate():
    m, feats, metas = load()
    ids = {r['@id'] for r in feats}
    ok = True
    # 1 уникальность
    print(f"@id уникальны: {len(ids)==len(feats)} ({len(feats)})")
    # 2 висячие
    dangling = [(r['@id'],k,t) for r in feats for k in ('dependsOn','sharesApi','feeds')
                for t in r.get(k,[]) if t not in ids]
    print(f"висячие ссылки: {len(dangling)}"); ok &= not dangling
    # 3 DAG
    adj = {r['@id']: r.get('dependsOn',[]) for r in feats}
    color={i:0 for i in adj}; cyc=[0]
    def dfs(u):
        color[u]=1
        for v in adj[u]:
            if color.get(v)==1: cyc[0]+=1
            elif color.get(v)==0: dfs(v)
        color[u]=2
    for i in adj:
        if color[i]==0: dfs(i)
    print(f"циклы dependsOn: {cyc[0]} (DAG={'да' if cyc[0]==0 else 'НЕТ'})"); ok &= cyc[0]==0
    # 4 unsafe-инвариант
    bad = [r['@id'] for r in feats if r.get('unsafe',0)>0 and 'переписать' not in r.get('notes','')]
    print(f"unsafe>0 без notes: {bad or 'нет'}"); ok &= not bad
    print("OK ✅" if ok else "FAIL ❌")
    return ok

def phases():
    m, feats, _ = load()
    adj = {r['@id']: r.get('dependsOn',[]) for r in feats}
    indeg = {i:0 for i in adj}
    for u in adj:
        for v in adj[u]:
            if v in indeg: indeg[v]+=1
    q = deque(sorted(i for i in adj if indeg[i]==0)); order=[]
    while q:
        u=q.popleft(); order.append(u)
        for v in adj[u]:
            if v in indeg:
                indeg[v]-=1
                if indeg[v]==0: q.append(v)
    print(f"порядок реализации ({len(order)} узлов):")
    for i, nid in enumerate(order,1): print(f"  {i:3d}. {nid}")

def stats():
    m, feats, metas = load()
    print(f"features={len(feats)} meta={len(metas)}")
    for f in ('source','category','priority','mechanic'):
        c = collections.Counter(r.get(f) for r in feats)
        print(f"  {f}: {dict(c.most_common())}")
    edges = collections.Counter()
    for r in feats:
        for k in ('dependsOn','sharesApi','feeds'): edges[k]+=len(r.get(k,[]))
    print(f"  edges: {dict(edges)}")

def hub():
    m, feats, _ = load()
    deg = collections.Counter()
    for r in feats:
        for k in ('dependsOn','sharesApi','feeds'):
            for t in r.get(k,[]): deg[r['@id']]+=1; deg[t]+=1
    print("топ-15 хабов:")
    for i,d in deg.most_common(15): print(f"  {d:2d}  {i}")

def turtle():
    m, feats, _ = load()
    for r in feats:
        s = r['@id']
        for pred in ('dependsOn','sharesApi','feeds'):
            for t in r.get(pred,[]):
                print(f"<{s}> <allspark:{pred}> <{t}> .")

if __name__ == '__main__':
    cmd = sys.argv[1] if len(sys.argv)>1 else 'stats'
    {'validate':validate,'phases':phases,'stats':stats,'hub':hub,'turtle':turtle}[cmd]()
