"""Recover KAS label names from their Cataclysm hashes (see kas_hash.py) by dictionary attack.

Inputs : recovered/missions/data/hashes.tsv   (written by kas_decompile.py --all --hashes ...)
Sources: extracted/ .mission/.missphere/.level/.script text (team/point/volume names ...),
         identifier-like strings in cataclysm.exe, HW1 KAS sources + kas2c output (labels, vars),
         the strings the missions themselves use (objective / ping / pointer labels).
Output : recovered/missions/data/kas_names.tsv  (hash, name, confidence, source)

confidence: 'exact'  - the name is a word from one of the sources (or a known kas2c pattern);
            'guess'  - built from 2 CamelCase tokens of that mission's vocabulary; collisions are
                       possible (28-bit hash), treat as a suggestion.
"""
import collections
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from kas_hash import kas_hash          # noqa: E402

ROOT = r'D:\Git\HWC_Modern'
DATA = os.path.join(ROOT, 'recovered', 'missions', 'data')
EXE = r'D:\Games\Homeworld Cataclysm\Cataclysm\cataclysm.exe'
IDENT = re.compile(r'[A-Za-z_][A-Za-z0-9_]{1,63}')


def words_from_text(txt):
    return set(IDENT.findall(txt))


def gather_sources():
    src = collections.defaultdict(set)     # word -> sources
    per_mission = collections.defaultdict(set)
    ext = os.path.join(ROOT, 'extracted')
    for dp, dn, fn in os.walk(ext):
        for f in fn:
            if not f.lower().endswith(('.mission', '.missphere', '.level', '.script', '.txt', '.dist')):
                continue
            p = os.path.join(dp, f)
            try:
                txt = open(p, encoding='latin1').read()
            except OSError:
                continue
            ws = words_from_text(txt)
            rel = os.path.relpath(p, ext)
            for w in ws:
                src[w].add('extracted')
            m = re.search(r'(mission\d+|tutorial\d+)', rel.lower())
            if m and f.lower().endswith(('.mission', '.missphere', '.level')):
                per_mission[m.group(1)] |= ws
    # exe strings
    d = open(EXE, 'rb').read()
    for m in re.finditer(rb'[\x20-\x7e]{3,}', d):
        for w in words_from_text(m.group().decode('latin1')):
            src[w].add('exe')
    # HW1 KAS + generated C
    for sub in (os.path.join(ROOT, 'reference', 'HomeworldSDL', 'src', 'Missions'),
                os.path.join(ROOT, 'reference', 'hw1-2003', 'src', 'Generated')):
        for dp, dn, fn in os.walk(sub):
            for f in fn:
                if f.lower().endswith(('.kas', '.c', '.h')):
                    txt = open(os.path.join(dp, f), encoding='latin1').read()
                    for w in words_from_text(txt):
                        src[w].add('hw1')
    return src, per_mission


def camel_tokens(w):
    return re.findall(r'[A-Z]+(?![a-z])|[A-Z]?[a-z]+|\d+', w)


def main():
    rows = [l.rstrip('\n').split('\t') for l in open(os.path.join(DATA, 'hashes.tsv'))][1:]
    targets = collections.defaultdict(set)    # hash -> kinds
    glob = {}                                  # hash -> True if used with kas2c's global (G_) flag
    by_mission = collections.defaultdict(set)
    for mission, h, kind, cnt in rows:
        hv = int(h, 16) & 0x7fffffff
        targets[hv].add(kind)
        if kind.startswith(('Var', 'Timer')):
            glob[hv] = glob.get(hv, False) or bool(int(h, 16) & 0x80000000)
        by_mission[mission].add(hv)
    src, per_mission = gather_sources()
    found = {}

    def try_name(w, conf, source):
        h = kas_hash(w)
        if h not in targets:
            return
        # kas2c marks G_ (global) variables/timers with bit 31: the name must agree
        isg = w[:2].upper() == 'G_'
        if isg != glob.get(h, False):
            return
        if h in found and found[h][1] == conf and found[h][0].lower() == w.lower():
            # same word, other spelling: prefer the CamelCase one (as written in the .missphere files)
            if sum(c.isupper() for c in w) > sum(c.isupper() for c in found[h][0]) and not w.isupper():
                found[h] = (w, conf, source)
            return
        if h in targets and (h not in found or (found[h][1] == 'guess' and conf == 'exact')):
            found[h] = (w, conf, source)

    # 1. single words and simple morphs
    for w, ss in src.items():
        s = ','.join(sorted(ss))
        if ss == {'exe'}:
            # the exe has ~10^5 random identifier-like strings: only plain, longer words
            if len(w) >= 5:
                try_name(w, 'exact', s)
            continue
        try_name(w, 'exact', s)
        try_name('G_' + w, 'exact', s + '+G_')
        for k in range(0, 21):
            try_name('%s%d' % (w, k), 'exact', s + '+digit')
            try_name('G_%s%d' % (w, k), 'exact', s + '+G_digit')
    # kas2c IFONCE variables
    for a in range(0, 200):
        for b in range(0, 200):
            try_name('_VIFONCE%d_%d' % (a, b), 'exact', 'kas2c-ifonce')
        try_name('_VIFONCE%03d' % a, 'exact', 'kas2c-ifonce')
    # 2. two-token combinations from each mission's vocabulary
    vocab_all = collections.Counter()
    for w, ss in src.items():
        if 'hw1' in ss or 'extracted' in ss:
            for t in camel_tokens(w):
                if len(t) >= 2 and not t.isdigit():
                    vocab_all[t[0].upper() + t[1:]] += 1
    for name in list(n for n, _, _ in found.values()):
        for t in camel_tokens(name):
            vocab_all[t[0].upper() + t[1:]] += 5
    vocab = [t for t, c in vocab_all.most_common(1500)]
    print('vocab', len(vocab), 'targets', len(targets), 'found exact', len(found))
    pre = {}
    for a in vocab:
        pre[a] = None
    for a in vocab:
        for b in vocab:
            w = a + b
            try_name(w, 'guess', 'combo')
            try_name('G_' + w, 'guess', 'combo')
    out = ['hash\tname\tconfidence\tsource\tkinds']
    for h in sorted(found):
        w, conf, s = found[h]
        out.append('%08x\t%s\t%s\t%s\t%s' % (h, w, conf, s, ','.join(sorted(targets[h]))))
    open(os.path.join(DATA, 'kas_names.tsv'), 'w', encoding='utf-8').write('\n'.join(out) + '\n')
    c = collections.Counter(v[1] for v in found.values())
    print('resolved %d of %d distinct hashes: %s' % (len(found), len(targets), dict(c)))
    kinds = collections.Counter()
    tot = collections.Counter()
    for h, ks in targets.items():
        for k in ks:
            tot[k] += 1
            if h in found and found[h][1] == 'exact':
                kinds[k] += 1
    for k, n in tot.most_common():
        print('  %-28s %4d/%4d exact' % (k, kinds[k], n))


if __name__ == '__main__':
    main()
