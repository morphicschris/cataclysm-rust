"""KAS label hash used by Homeworld: Cataclysm (cataclysm.exe 0x0048ce80).

Cataclysm's kas2c replaced KAS string labels (FSM/state names, variables, timers,
teams, selections, volumes, paths, points, objectives ...) with 32-bit hashes. The
runtime hashes the label strings it reads from .mission/.missphere files with the
same function, so mission code and level data meet on the hash value.

    h = 0
    for c in name:
        h = (h << 4) + (c & 0xDF)          # &0xDF folds a-z onto A-Z (and shifts digits)
        hi = h & 0xF0000000
        if hi:
            h ^= hi >> 24
            h &= ~hi                       # (asm does h ^= hi ^ (hi >> 24))
    -> always < 0x10000000

kas2c ORs 0x80000000 into the constant for a *global* (mission-scope independent)
variable/timer; the runtime (kasfVar*/kasfTimer*) strips it and uses scope 0.

Usage:
    kas_hash.py NAME [NAME ...]          print hashes
    kas_hash.py -f FILE                  hash every word in a file
"""
import re
import sys


def kas_hash(name):
    h = 0
    for ch in name.encode('latin1'):
        h = ((h << 4) + (ch & 0xDF)) & 0xFFFFFFFF
        hi = h & 0xF0000000
        if hi:
            h ^= hi ^ (hi >> 24)
    return h


GLOBAL_FLAG = 0x80000000


def main(argv):
    if len(argv) >= 2 and argv[0] == '-f':
        words = set(re.findall(r'[A-Za-z_][\w]*', open(argv[1], encoding='latin1').read()))
    else:
        words = argv
    for w in sorted(words):
        print('0x%08x %s' % (kas_hash(w), w))


if __name__ == '__main__':
    main(sys.argv[1:])
