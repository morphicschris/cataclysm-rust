"""Compile failing HW1 files one by one via recovered/hw1build/build.cmd and tally the first error of each."""
import collections, subprocess, sys

root = r'D:\Git\HWC_Modern\recovered\hw1build'
files = [l.strip() for l in open(root + r'\fail.txt') if l.strip()]
limit = int(sys.argv[1]) if len(sys.argv) > 1 else len(files)
tally, examples = collections.Counter(), {}
for f in files[:limit]:
    out = subprocess.run(['cmd', '/c', root + r'\build.cmd', f], capture_output=True, text=True, errors='replace').stdout
    first = next((l for l in out.splitlines() if ' error ' in l), 'no error?')
    key = first.split(' error ', 1)[-1][:110]
    tally[key] += 1
    examples.setdefault(key, (f, first[:200]))
for k, n in tally.most_common(25):
    print('%3d  %s\n      e.g. %s' % (n, k, examples[k][1]))
