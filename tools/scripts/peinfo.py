import struct,sys,datetime
p=sys.argv[1]; d=open(p,'rb').read()
pe=struct.unpack_from('<I',d,0x3c)[0]
mach,nsec,ts,_,_,optsz,ch=struct.unpack_from('<HHIIIHH',d,pe+4)
o=pe+24
magic,lmaj,lmin=struct.unpack_from('<HBB',d,o)
ep,=struct.unpack_from('<I',d,o+16); ib,=struct.unpack_from('<I',d,o+28)
sub,=struct.unpack_from('<H',d,o+68)
print(p, 'machine',hex(mach),'time',datetime.datetime.utcfromtimestamp(ts),'linker',lmaj,lmin,'ep',hex(ib+ep),'imagebase',hex(ib),'subsys',sub)
dd=o+96
dirs=[struct.unpack_from('<II',d,dd+8*i) for i in range(16)]
s=o+optsz; secs=[]
for i in range(nsec):
    n,vs,va,rs,rp=struct.unpack_from('<8sIIII',d,s+40*i); secs.append((n.rstrip(b'\0').decode(),va,vs,rp,rs)); print('  sec',n.rstrip(b'\0').decode(),hex(va),hex(vs),'raw',hex(rs))
def off(rva):
    for n,va,vs,rp,rs in secs:
        if va<=rva<va+max(vs,rs): return rva-va+rp
irva=dirs[1][0]
if irva:
    i=off(irva)
    while True:
        ilt,_,_,name,iat=struct.unpack_from('<IIIII',d,i)
        if name==0: break
        dll=d[off(name):d.index(b'\0',off(name))].decode()
        t=off(ilt or iat); fns=[]
        while True:
            e,=struct.unpack_from('<I',d,t)
            if e==0: break
            if e&0x80000000: fns.append('#%d'%(e&0xffff))
            else:
                q=off(e)+2; fns.append(d[q:d.index(b'\0',q)].decode())
            t+=4
        print('  import',dll,len(fns), ' '.join(fns) if len(fns)<=60 else ' '.join(fns[:60])+' ...')
        i+=20
erva=dirs[0][0]
if erva:
    e=off(erva); nn,=struct.unpack_from('<I',d,e+24); print('  exports',nn)
print('  debugdir',dirs[6], 'reloc',dirs[5])
rest=len(d)-max(rp+rs for *_,rp,rs in secs); print('  overlay bytes',rest)
