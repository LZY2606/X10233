C=299792458.0; f1=1575.42e6; f2=1227.60e6
w1=C/f1; w2=C/f2; g=(f1/f2)**2; wlf=C/(f1-f2)
R0=20_000_000.0
rate={'G07':31.0,'G09':17.0,'G11':-43.0,'G30':8.0}
def R(sat,i): return R0 + rate[sat]*i
def ION(i): return 12.0
sats=['G07','G09','G11','G30']
amb={('G07',1):123456.0,('G07',2):96000.0,('G09',1):111111.0,('G09',2):87000.0,
     ('G11',1):222222.0,('G11',2):173000.0,('G30',1):150000.0,('G30',2):117000.0}
durs=[None,20,22,19,70,17,30,17,25]
t=0.0; times=[t]
for d in durs[1:]: t+=d; times.append(t)
jumps={}
def jump(sat,i,a1,a2): jumps[(sat,i)]=(a1,a2)
jump('G11',4,2,0)      # single-sat slip at clock epoch
jump('G07',6,1,0)      # single-sat MW+TD supported slip
jump('G07',8,0.5,0.0)  # genuine half-grid ambiguity tie
jump('G09',7,0.42,0.0) # source typo (0.080m GF, below v1 trigger), corrected locally
clock_epoch=4; clock_step=1000.0
no_epoch={('G30',5),('G30',6)}
no_field=set()
for L in (1,2): no_field.add(('G07',8,f'C{L}W'))
no_field.add(('G30',9,'C2W'))
bad=('G30',9,'C1W')
lli={('G07',7,1)}
def cn(sat,i,L):
    if sat=='G30' and i==9 and L==1: return 28.5
    return (44.0 if L==1 else 39.0)+(1 if sat=='G11' else 0)-(i*0.05)
def cum(sat,i,L):
    a=amb[(sat,L)]
    for (s,j),(a1,a2) in jumps.items():
        if s==sat and j<=i: a += a1 if L==1 else a2
    return a
def P(sat,i,L):
    v=R(sat,i)+(ION(i) if L==1 else g*ION(i))
    if i>=clock_epoch: v+=clock_step
    return v
def Lc(sat,i,L):
    ion=ION(i); w=w1 if L==1 else w2
    v=(R(sat,i)-(ion if L==1 else g*ion))/w + cum(sat,i,L)
    if i>=clock_epoch: v+=clock_step/w
    return v
lines=['REL 1.0',
 'STATION SITE01 35.681 139.767 40.0 ITRF2014',
 'FREQ 1 L1 1575.420 MHz GPS C1W L1W D1W S1W',
 'FREQ 2 L2 1227.600 MHz GPS C2W L2W D2W S2W',
 'NOTE carrier-break-station offline case; all values synthetic, virtual time']
for i in range(1,10):
    sec=times[i-1]
    lines.append(f'* 2026 09 24 00 {int(sec//60):02d} {sec%60:06.3f}  GPS  {len(sats)}')
    for sat in sats:
        if (sat,i) in no_epoch: continue
        toks=[]
        for L in (1,2):
            for kind in (f'C{L}W',f'L{L}W',f'D{L}W',f'S{L}W'):
                if (sat,i,kind) in no_field: toks.append('')
                elif (sat,i,kind)==bad: toks.append('999.999!')
                elif kind.startswith('C'): toks.append(f'{P(sat,i,L):.3f}')
                elif kind.startswith('L'):
                    val=Lc(sat,i,L)
                    toks.append(f'{val:.4f}{("1" if (sat,i,L) in lli else "")}')
                elif kind.startswith('D'):
                    w=w1 if L==1 else w2
                    toks.append(f'{-rate[sat]/w:.4f}')
                else: toks.append(f'{cn(sat,i,L):.1f}')
        lines.append(sat+' '+' '.join(toks))
lines.append(f'CLOCK 2026 09 24 00 {int(times[clock_epoch-1]//60):02d} {times[clock_epoch-1]%60:06.3f} RECEIVER +1000.0 m STEP NOTE site_log:receiver_clock_jump')
lines.append('SATSTATE G11 2026 09 24 00 00 00.000 HEALTHY EL 32.0 AZ 210.0')
lines.append(f'SATSTATE G30 2026 09 24 00 {int(times[6]//60):02d} {times[6]%60:06.3f} HEALTHY EL 8.5 AZ 150.0')
open('cases/carrier-break-station.rel','w').write('\n'.join(lines)+'\n')

def obs(i):
    out={}
    for sat in sats:
        if (sat,i) in no_epoch: continue
        for L in (1,2):
            cd=None if (sat,i,f'C{L}W') in no_field else P(sat,i,L)
            ph=None if (sat,i,f'L{L}W') in no_field else Lc(sat,i,L)
            out[(sat,L)]=(cd,ph)
    return out
def mw(o):
    (C1,L1),(C2,L2)=o[1],o[2]
    return (wlf*(L1-L2)-(f1*C1-f2*C2)/(f1-f2))/wlf
print("boundary expectations (GF dI m, MW dNw, TD1 m):")
for i in range(2,10):
    a=obs(i-1); b=obs(i); dt=durs[i-1]
    for sat in sats:
        if (sat,i) in no_epoch or (sat,i-1) in no_epoch: continue
        if (sat,1) not in a or (sat,1) not in b: continue
        dI=dN=td=None
        Ia=w1*a[(sat,1)][1]-w2*a[(sat,2)][1]; Ib=w1*b[(sat,1)][1]-w2*b[(sat,2)][1]; dI=Ib-Ia
        if a[(sat,1)][0] is not None and b[(sat,1)][0] is not None and a[(sat,2)][0] is not None and b[(sat,2)][0] is not None:
            dN=mw({1:b[(sat,1)],2:b[(sat,2)]})-mw({1:a[(sat,1)],2:a[(sat,2)]})
        if a[(sat,1)][0] is not None and b[(sat,1)][0] is not None:
            td=w1*(b[(sat,1)][1]-a[(sat,1)][1])-(b[(sat,1)][0]-a[(sat,1)][0])
        f=lambda x:'  --  ' if x is None else f'{x:7.3f}'
        print(f'e{i-1}->e{i} {sat} dt={dt:3d} GF={f(dI)} MW={f(dN)} TD1={f(td)}{" LLI" if (sat,i,1) in lli else ""}')
