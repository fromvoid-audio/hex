#!/usr/bin/env python3
"""Generate editable Frame geometry; synthetic content, never a shipped UI."""
import json
import math
import colorsys
from pathlib import Path
from clip_palette import contrast
from theme_palette import THEME
OUT = Path(__file__).parent
W, H = 1900, 1120
C = dict(bg=THEME['background'],panel=THEME['surface'],dark=THEME['well'],grid=THEME['separator'],text=THEME['text'],muted=THEME['muted'],coral='#b39b8a',pink='#ac97a2',purple='#a39cad',blue='#8f9fad',cyan='#96ada8',amber=THEME['orange'],yellow='#aca68c',enabled=THEME['enabled'],focus=THEME['sea_glass'],tertiary=THEME['violet'],electric=THEME['blue'])
# Concept artwork colors follow the UI anchors, independent of user clip choices.
CONCEPT_COLORS = {key:'#454c49' for key in ['coral','pink','purple','blue','cyan','yellow']}
C.update(CONCEPT_COLORS)
PICKER = dict(mode='custom', maxSavedSwatches=64, selectedColor=THEME['orange'],
              savedSwatches=[THEME[k] for k in ['violet','blue','sea_glass','orange']]+
                            ['#c6c9c4','#6c7472','#363a39','#171b1b'],
              controls=['saturation/value','hue','hex','rgb','save swatch'],
              persistence='design specification; native editing and persistence deferred')
(OUT/'clip-colors.json').write_text(json.dumps(PICKER,indent=2)+'\n')
def clip_ink(background):
    return max(['#060909','#f2f5ef'],key=lambda ink:contrast(background,ink))

nodes=[]; positions={}; serial=0; parent=None
meter_paint=[]

def node(kind,x,y,w,h,text=None,fill='none',stroke='none',color=None,size=11,id=None,par=None,**extra):
    global serial
    serial+=1; id=id or f'n-{serial}'; par=parent if par is None else par
    px,py=positions.get(par,(0,0)); style=dict(fill=C.get(fill,fill),stroke=C.get(stroke,stroke),color=C.get(color or 'text',color or C['text']),fontSize=size,fontFamily='Ioskeley Mono',padding=0)
    style.update(extra.pop('style',{}))
    # Labels adapt to the muted color fill; utility wells keep theme typography.
    if color=='dark':
        background=style['fill']
        if background=='none' and par:
            background=next((n['style']['fill'] for n in reversed(nodes) if n['id']==par),'none')
        if background in CONCEPT_COLORS.values():style['color']=clip_ink(background)
    n=dict(id=id,type=kind,bounds=dict(x=x-px,y=y-py,width=w,height=h),style=style,**extra)
    if par: n['parentId']=par
    if text is not None: n['text']=text
    nodes.append(n); positions[id]=(x,y); return id

def rect(x,y,w,h,fill='panel',stroke='grid',id=None,**kw):return node('container',x,y,w,h,fill=fill,stroke=stroke,id=id,**kw)
def txt(x,y,w,text,size=11,color='text',**kw):return node('text',x,y,w,max(12,size*1.4),text=text,size=size,color=color,**kw)
def line(x,y,w,h=0,color='grid',points=None,**kw):
    if points is None:
        points=[[max(0,-w),max(0,-h)],[max(0,w),max(0,h)]]
        x+=min(0,w);y+=min(0,h);w=abs(w);h=abs(h)
    return node('line',x,y,w,h,stroke=color,points=points,**kw)
def box(x,y,w,text,fill='dark',size=10,h=18,**kw):return node('container',x,y,w,h,text=text,fill=fill,stroke=THEME['field_border'],size=size,style=dict(padding=1,verticalAlign='middle'),**kw)
def tagrow(x,y,tags):
    """Individual compact tag outlines, keeping the existing browser row positions."""
    for tag in tags:
        width=len(tag)*5.4+8
        node('container',x,y,width,15,text=tag,fill=THEME['tag_fill'],stroke=THEME['tag_border'],size=9,
             id='tag-'+str(round(y))+'-'+str(round(x)),style=dict(padding=1,verticalAlign='middle'))
        x+=width+3


def action(id,x,y,w,text,state,label):return node('button',x,y,w,20,text=text,fill='dark',stroke='grid',size=12,id=id,label=label,action=dict(type='set-state',state=state),style=dict(padding=0,textAlign='center',verticalAlign='middle'))
def knob(x,y,r=12,value=.6,color='text'):
    k=rect(x-r,y-r,2*r,2*r,'panel','dark',style=dict(radius=r)); a=2.4+value*4.6
    line(x,y,math.cos(a)*(r-3),math.sin(a)*(r-3),color,par=k)
def meter(x,y,w,h,seed,par=None):
    well=rect(x,y,w,h,'dark','none',par=par)
    lanes=2 if w>=6 else 1
    levels=[.43,.68,.55,.79,.36,.63,.95,.58,.72,.48,.66,.31]
    for lane in range(lanes):
        level=min(.99,levels[seed%len(levels)]+(.04 if lane==0 else -.05))
        width=(w-3)/2 if lanes==2 else max(1,w-2)
        bx=x+1+lane*(width+1)
        lane_paint_start=len(meter_paint)
        fill_height=level*(h-2)
        bar=rect(bx,y+h-1-fill_height,width,fill_height,THEME['sea_glass'],'none',par=well)
        meter_paint.append(bar)
        if level>.80:
            cap_height=(level-.80)*(h-2)
            cap=rect(bx,y+h-1-fill_height,width,cap_height,THEME['orange'],'none',par=bar)
            meter_paint.append(cap)
        if level>.94:
            red_height=(level-.94)*(h-2)
            tip=rect(bx,y+h-1-fill_height,width,red_height,THEME['error'],'none',par=cap)
            meter_paint.append(tip)
        peak=rect(bx,max(y+1,y+h-2-level*(h-2)-5),width,1,THEME['error'] if level>.94 else THEME['orange'] if level>.80 else THEME['sea_glass'],'none',par=well)
        nodes[-1]['allowOverlapWith']=meter_paint[lane_paint_start:]
        meter_paint.append(peak)
    return well

def wave(x,y,w,h,seed=0):
    for i in range(1,int(w),1):
        a=(.08+.52*abs(math.sin((i+seed)*1.83))*math.exp(-((i%28)/18)))*h/2
        line(x+i,y+h/2-a,0,a*2,THEME['waveform'])
def clip(x,y,w,h,color,name='',seed=0,midi=False):
    k=rect(x,y,w,h,color,'grid')
    if name: txt(x+3,y+2,w-6,name if len(name)*6<=w-6 else name[:max(1,int((w-6)/6))],10,'dark',par=k)
    if midi:
        for i in range(2,int(w)-3,7):line(x+i,y+19+(i%3)*6,3,0,THEME['waveform'],par=k)
    else:
        old=globals()['parent'];globals()['parent']=k;wave(x,y+17,w,h-18,seed);globals()['parent']=old

# Shared chrome: screenshot-sized product canvas without the surrounding black margin.
rect(0,0,W,H,'bg','muted',id='shell',style=dict(radius=10))
parent='shell'
for x,c in [(14,'coral'),(35,'amber'),(56,'cyan')]:rect(x,10,12,12,c,'none',style=dict(radius=6))
txt(900,10,120,'hex · e04',12)
# Transport groups track the reference's spacing, tiny fields, and icon-led controls.
box(12,31,25,'‖','amber',12,color='dark',h=20)
for x,w,t in [(55,58,'link ▾'),(115,28,'tap'),(145,46,'143.00'),(193,36,'▥ ▥'),(231,43,'4 / 4'),(276,40,'100%'),(318,40,'● ○'),(360,52,'1 bar ▾'),(432,24,'♯'),(458,44,'c ▾'),(504,74,'major ▾')]:box(x,31,w,t)
box(701,31,22,'↦');box(727,31,90,'3 . 1 . 3')
action('play',825,30,22,'▶','playing','play · space')
action('pause',825,30,22,'‖','default','pause · space')
action('stop',850,30,22,'■','default','stop · escape')
box(878,31,22,'●')
for x,t in [(915,'+'),(938,'⌁'),(961,'←'),(984,'□'),(1007,'○')]:box(x,31,21,t,'amber' if x==938 else 'dark',color='dark' if x==938 else 'text')
for x,w,t in [(1051,86,'17 . 1 . 1'),(1140,22,'⌁'),(1165,22,'▱'),(1190,22,'⌁'),(1215,87,'4 . 0 . 0')]:box(x,31,w,t)
for x,w,t in [(1590,22,'✎'),(1615,22,'▥'),(1640,32,'key'),(1675,32,'midi'),(1710,51,'44.1khz'),(1764,55,'27% ▾')]:box(x,31,w,t)
action('session-tab',1845,30,20,'▥','session','session view · tab')
action('arrange-tab',1870,30,20,'≡','default','arrangement view · tab')
# Browser is shared all the way down to the groove pool, as in both references.
rect(10,65,418,908,'panel','grid',id='browser');parent='browser'
box(14,68,22,'‹');box(38,68,22,'›');box(62,68,325,'search (cmd + f)',size=11);box(391,68,24,'▤','amber',color='dark')
rect(10,93,176,880,'panel','grid',id='browser-nav')
txt(15,97,165,'collections',10,'muted')
for i,(t,c) in enumerate([('kick','coral'),('percs','amber'),('impact','yellow'),('artifact','cyan'),('texture','blue'),('lowend','purple'),('racks','muted')]):
    rect(20,122+i*20,7,7,c,'none');txt(37,118+i*20,144,t)
txt(15,276,160,'library',10,'muted')
for i,t in enumerate(['▥ all','☷ midi','⌁ mod','♧ m4l','▣ plugins','≈ grooves','▤ templates','⌁ distortion','▥ delay','◉ reverb','♧ mixing','⌁ filter','▤ synth','▣ audio effects','▣ samples']):
    row=None
    if i==0:
        row=rect(11,297+i*20,174,20,THEME['selection_fill'],'none')
        rect(11,297+i*20,2,20,'amber','none',par=row)
    txt(17,299+i*20,166,t,11,'amber' if i==0 else 'text',par=row or parent)
txt(15,619,160,'places',10,'muted')
for i,t in enumerate(['▣ packs','♧ splice','♙ user library','▤ current project','□ inbox','□ core','□ 3rdparty','□ unsorted','□ clips','□ portalsurfer','□ music','□ references','□ allprojects','□ allsamples','□ add folder…']):txt(17,642+i*20,166,t)
line(187,94,0,878)
txt(194,97,225,'content ▾',11)
for row,tags in enumerate([['audio clip','backup set','device'],['groove','image','midi clip','midi file'],['midi tool','preset','sample','set'],['tuning','video']]):tagrow(194,119+row*17,tags)
txt(194,198,224,'type ▾',11)
for row,tags in enumerate([['loop','one shot','mpe enabled'],['multisampled','impulse response']]):tagrow(194,219+row*17,tags)
txt(194,263,224,'sounds ▾',11)
for row,tags in enumerate([['bass ›','brass ›','guitar & plucked ›'],['lead','mallets ›','pad'],['piano & keys ›','strings ›','voice ›'],['woodwind ›','ambience & fx ›'],['artifact','bass']]):tagrow(194,282+row*17,tags)
txt(194,375,224,'drums ▾',11);tagrow(194,396,['drum kit ›','kick','snare ›','clap'])
box(190,419,235,'name                                    ◉',size=9)
items=['cv instrument','cv lfo','cv shaper','cv triggers','cv utility','delay','drift','drum buss','drum rack','drum sampler','ds clang','ds clap','ds cymbal','ds fm','ds hh','ds kick','ds snare','ds tom','dynamic tube','echo','electric','envelope follower','envelope midi','eq eight','eq three']
for i,t in enumerate(items):
    row=None
    if t=='eq eight':
        row=rect(190,440+i*19,235,19,THEME['selection_fill'],'none')
        rect(190,440+i*19,2,19,'amber','none',par=row)
    txt(195,441+i*19,220,'▸ ▤ '+t,11,'text',par=row or parent)
txt(192,922,36,'tags:',10,'muted');tagrow(230,922,['eq']);txt(257,923,80,'add…',10,'muted');txt(270,949,150,'1 item selected',10,'muted')
parent='shell';rect(10,982,418,106,'panel','grid',id='groove');parent='groove'
box(12,984,413,'groove name         base   quantize timing random velocity',size=9)
for i,t in enumerate(['swing 16ths…','swing 32nds…','swing 16ths…']):
    y=1006+i*20;txt(15,y,128,t,10);box(145,y,47,'1/16 ▾',size=9);box(197,y,45,'0%',size=9);box(247,y,45,'100%','amber',size=9,color='dark');box(297,y,45,'0%',size=9);box(347,y,75,'100% ▾',size=9)
txt(14,1070,170,'groove pool',10,'muted');txt(280,1070,140,'global amount 100%',10)
parent='shell';box(44,1097,1290,'design study · synthetic clips / illustrative devices','panel',11,color='muted',h=19)
txt(1390,1099,240,'22 · portal_kick_038',10);box(1640,1097,225,'clip / device detail',size=10)
# Arrangement: overview, timeline, right headers, and shallow device detail.
rect(437,65,1452,1023,'panel','grid',id='arrangement');parent='arrangement'
rect(437,65,1452,16,'dark','muted',id='overview')
for j,c in enumerate(['blue','purple','coral','pink']):line(442,68+j*3,840+j*90,0,c,par='overview')
rect(439,66,465,14,'none','muted',par='overview')
rect(437,83,1120,758,'panel','grid',id='timeline')
for i in range(20):
    x=438+i*56;line(x,105,0,720,'grid',par='timeline');txt(x+4,89,50,str(1+i*8),10,'muted',par='timeline')
box(1570,87,33,'set',size=10);box(1610,87,26,'⌁');box(1640,87,23,'▣','coral',color='dark')
for i,h in enumerate([61,40,91,20,91,94,54,92,40]):
    y=126+sum([61,40,91,20,91,94,54,92,40][:i]);line(438,y+h,1118)
    color=['coral','coral','pink','coral','coral','blue','coral','coral','coral'][i]
    name=['24 hive','25 sift-01791417','26 twics1_pad_1','27 botanical-liquid','28 sift-01791418','29 sift-01791418','30 drum rack','31 audio','32 audio'][i]
    head=rect(1559,y,120,h,color,'grid');txt(1563,y+2,114,name,9,'dark',par=head)
    for q,t in enumerate(['no input ▾','tex 5 ▾'][:2 if h>40 else 1]):box(1682,y+2+q*19,76,t,size=9,h=17)
    box(1764,y+4,44,str(24+i),'enabled',9,color='dark',h=17);box(1812,y+4,20,'s',size=9,h=17)
    if h>40: box(1764,y+23,44,['-6.7','-13.8','-20.9','-12.0','-12.0','-17.4','-26.1','-12.0','-12.0'][i],size=9,h=17)
    if h>70:
        for z in range(2 if h>78 else 1):
            for q in range(3):box(1764+q*35,y+43+z*17,31,'−∞',size=9,h=16)
    meter(1865,y+3,6,h-6,i)
    if i==0:
        for n in range(33):clip(465+n*28,y,9,h,'coral',seed=n)
    if i==1:
        for n in range(35):line(465+n*29,y+2,0,h-4,'coral')
    if i==2:
        for n in range(19):clip(444+n*57,y,21,h,'pink','tw',n)
    if i==3:clip(438,y,1117,h,'coral','846618 · cat-fox · botanical-liquid-texture-1')
    if i==4:
        for n in [2,3,4,5,18,19,20,21]:clip(438+n*28,y,25,h,'coral','sift',midi=True)
    if i==5:
        for n in range(19):clip(635+n*29,y,7,h,'blue',seed=n)
    if i==6:clip(438,y,1117,h,'coral','drum rack',midi=True)
for i,(t,c) in enumerate([('a room','yellow'),('b darkverb','yellow'),('c echo','yellow'),('d dirt','cyan'),('e crush','coral'),('f density','purple'),('main','muted')]):
    y=710+i*20;rect(1559,y,120,20,c,'grid');txt(1563,y+2,112,'▷ '+t,10,'dark');box(1764,y+1,44,chr(97+i) if i<6 else '0','enabled' if i<6 else 'focus',9,color='dark');box(1812,y+1,20,'s',size=9);box(1835,y+1,30,'post' if i<6 else '0',size=9)
    line(438,y,1118)
for i in range(14):txt(440+i*80,842,65,f'{i//3}:{(i%3)*20:02}',10,'muted')
box(1690,850,54,'1.00×',size=9);box(1750,850,24,'h');box(1778,850,24,'w')
# Device dock, same narrow rack strip and two adjacent devices as reference.
rect(437,870,1452,218,'panel','grid',id='devices');parent='devices'
rect(439,872,22,213,'dark','muted');txt(444,890,14,'◉',10,'cyan')
rect(469,873,157,212,'panel','grid',id='verb');box(471,874,153,'● ◉ ↗ valhalla…',size=11)
rect(476,901,143,159,'dark','grid');box(476,1065,69,'none ▾',size=9);box(550,1065,69,'none ▾',size=9)
rect(635,873,504,215,'panel','grid',id='eq');box(637,874,500,'● ↟ eq eight                                     ◉ ▣',size=11)
rect(691,900,390,143,'dark','grid',id='eq-chart')
for j in range(20):line(693+j*20,900,0,141,'grid',par='eq-chart')
for j in range(6):line(692,905+j*26,388,0,'grid',par='eq-chart')
line(700,900,374,143,'focus',points=[[x,143-135/(1+math.exp(-(x-245)/30))] for x in range(0,375,4)],par='eq-chart')
for y,t in [(901,'freq'),(956,'gain'),(1011,'q')]:txt(641,y,45,t,10);knob(661,y+25,10,.5,'focus')
for j,t in enumerate(['2.30khz','0.00db','0.41']):txt(640,939+j*55,48,t,8)
for j in range(8):box(694+j*49,1050,44,'⌁ ▾',size=9);box(694+j*49,1070,44,str(j+1),size=9)
for j,t in enumerate(['mode','stereo','edit','l','adapt. q','on','scale','100%','output','0.00db']):box(1086,900+j*18,49,t,'amber' if t=='on' else 'dark',size=9,h=17,color='dark' if t=='on' else 'text')
txt(1440,969,260,'drop audio effects here',11,'muted')
# Session: high clip matrix, full-height channel strips, narrow return bank.
parent='shell';rect(437,65,1452,1023,'panel','grid',id='session-view');parent='session-view'
colors=['purple']*5+['coral']*3+['pink']+['coral']*4
names=['14 group','15 rk_tribal_k','16 rk_tribal_k','17 rk_tribal_k','18 rk_tribal_k','tex 5','20 hive','21 sift-01791414','22 portal_kick','23 sift-01791414','24 sift-01791414','25 sift-01791416']
for i in range(12):
    x=437+i*100; c=colors[i]; strip=rect(x,65,100,651,'panel','grid',id=f'strip-{i}')
    for q in range(4):line(x+1,67+q*5,98,0,c,par=strip)
    head=rect(x,88,100,23,c,'grid',par=strip);txt(x+3,91,94,names[i],9,'dark',par=head)
    for row in range(6):
        y=113+row*20;slot=rect(x,y,100,20,'panel','grid',par=strip)
        active=(i==3 and row in [1,2,3]) or (i==8 and row in [1,2,5]) or (i==6 and row==0)
        if active:
            rect(x+1,y+1,98,18,c,'none',par=slot);txt(x+4,y+1,18,'▶',10,'dark',par=slot)
        else:rect(x+6,y+6,7,7,'dark','none',par=slot)
    rect(x,234,100,38,'panel','grid',par=strip)
    box(x+4,275,91,'■      │',size=10,par=strip)
    txt(x+5,296,91,'midi from' if i%2 else 'audio from',10,par=strip)
    box(x+4,312,92,'all ins ▾' if i%2 else 'ext. in ▾',size=10,par=strip)
    box(x+4,331,92,'all channels ▾' if i%2 else '1/2 ▾',size=9,par=strip)
    txt(x+5,351,90,'monitor',10,par=strip)
    box(x+4,367,23,'in',size=9,par=strip);box(x+29,367,38,'auto','enabled',9,color='dark',par=strip);box(x+69,367,27,'off',size=9,par=strip)
    txt(x+5,388,90,'audio to',10,par=strip);box(x+4,404,92,'14-group ▾' if i<5 else 'tex 5 ▾',size=10,par=strip)
    box(x+4,425,92,'',size=9,h=12,par=strip)
    line(x,442,100,par=strip);txt(x+51,449,45,'sends',10,par=strip)
    for q in range(6):
        kx=x+27+(q%2)*34;ky=464+(q//2)*34+(q%2)*17;old=parent;parent=strip;knob(kx,ky,12,.18);parent=old;txt(kx+14,ky-4,8,chr(97+q),9,par=strip)
    line(x,573,100,par=strip)
    box(x+8,582,38,'−∞',size=9,h=17,par=strip);box(x+8,602,38,['-4.2','0','-4.6','-1.1','-6.6','0','-17.3','-16.3','-12.0','-9.8','-16.3','-30.4'][i],size=9,h=17,par=strip)
    old=parent;parent=strip;knob(x+27,639,12,.5);parent=old
    box(x+17,659,23,str(14+i),'focus' if i==8 else 'enabled',10,color='dark',h=23,par=strip);box(x+17,683,23,'s',size=9,h=15,par=strip);box(x+17,699,23,'●','pink' if i==8 else 'dark',7,color='dark' if i==8 else 'muted',h=12,par=strip)
    meter(x+57,582,12,121,i,par=strip);line(x+54,602+(i%5)*10,18,0,'text',par=strip)
    for q in range(7):txt(x+74,587+q*16,22,str(q*12),8,'muted',par=strip)
    rect(x+3,711,94,5,c,'none',par=strip)
# Return strips are intentionally narrow, leaving the scene/master column intact.
for i,c in enumerate(['yellow','yellow','yellow','cyan','coral','purple']):
    x=1637+i*24; r=rect(x,65,24,651,'panel','grid')
    rect(x+1,66,22,45,c,'none',par=r);txt(x+7,91,14,chr(97+i),10,'dark',par=r)
    for row in range(6):line(x,113+row*20,24,par=r)
    meter(x+7,582,5,73,i+3,par=r);line(x+3,593,12,0,'text',par=r)
    box(x+4,659,16,chr(97+i),'enabled',9,color='dark',h=23,par=r);box(x+4,686,16,'s',size=9,h=17,par=r)
    rect(x+1,711,22,5,c,'none',par=r)
rect(1781,65,108,651,'panel','grid',id='master');txt(1786,90,95,'main',10,par='master')
for row in range(6):
    y=113+row*20
    action(f'scene-{row}',1784,y,102,f'▷              {row+1}','session-playing',f'launch scene {row+1}')
    nodes[-1]['parentId']='master';nodes[-1]['bounds']['x']=3;nodes[-1]['bounds']['y']=y-65;nodes[-1]['style']['fontSize']=9
for j,t in enumerate(['cue out','1/2 ▾','main out','1/2 ▾']):box(1786,349+j*19,95,t,size=10,par='master')
for q in range(6):box(1800+(q%2)*34,459+(q//2)*34,31,'post','enabled',9,color='dark',par='master')
box(1795,582,36,'−∞',size=9,par='master');box(1795,602,36,'0',size=9,par='master')
old=parent;parent='master';knob(1813,641,12,.5);knob(1813,694,12,.5,'blue');parent=old
meter(1853,582,12,122,6,par='master')
for q in range(7):txt(1868,587+q*16,20,str(q*12),8,'muted',par='master')
rect(437,723,1452,12,'dark','grid');rect(710,726,282,6,'muted','none',style=dict(radius=3))
# Clip detail: parameter panel left, piano roll right, tabs flush with panel edge.
rect(437,742,1452,346,'panel','muted',id='clip-detail');parent='clip-detail'
box(439,744,358,'▣ clip','pink',11,h=20,color='dark');box(800,744,28,'⌕',h=20);box(832,744,40,'fold',h=20)
for x,w,t in [(1240,65,'notes'),(1307,98,'envelopes'),(1407,54,'mpe'),(1815,69,'1/16 ▾')]:box(x,744,w,t,fill='tertiary' if t=='notes' else 'dark',size=10,h=20,color=clip_ink(THEME['violet']) if t=='notes' else 'text')
for j,t in enumerate(['▣','▸','☷','⌁','◉']):box(440+j*70,768,68,t,h=18)
for y,a,b in [(790,'start','end'),(871,'position','length'),(913,'signature','groove'),(954,'scale','')]:
    txt(446,y,83,a,10);txt(536,y,84,b,10)
    box(446,y+18,82,'1 . 1 . 1' if y<900 else '4 / 4',size=9);box(536,y+18,83,'2 . 1 . 1' if y<900 else 'none ▾',size=9)
box(446,830,173,'duplicate',size=10);box(446,851,173,'⌁ loop','amber',10,color='dark')
box(446,995,173,'c ▾       major ▾',size=10)
line(632,786,0,298)
box(642,790,76,'g♯3','dark',12,h=38,color='text');box(724,790,65,'fit scale',size=9);box(724,810,65,'invert',size=9)
for j,t in enumerate(['0 st','interval','stretch','×2','/2','grid','set length','10%','humanize','reverse','legato']):
    x=642 if j%2==0 else 724;y=838+(j//2)*34;box(x,y,76 if j%2==0 else 65,t,size=9)
rect(804,786,1084,301,'panel','grid',id='piano-roll')
for i in range(32):
    y=809+i*8.5;rect(804,y,1083,8.5,THEME['piano_row'] if i%12 in [1,3,6,8,10] else 'panel','none',par='piano-roll')
    rect(804,y,28,8.5,THEME['key_dark'] if i%12 in [1,3,6,8,10] else THEME['key_light'],'none',par='piano-roll')
for i in range(16):
    x=832+i*66;line(x,806,0,280,'grid',par='piano-roll')
    if i%2==0:txt(x+4,790,120,['1','1.1.3','1.2','1.2.3','1.3','1.3.3','1.4','1.4.3','2'][i//2],9,'muted',par='piano-roll')
rect(832,809,1055,10,'pink','muted',par='piano-roll')
for x,y,w,t in [(898,964,64,'f♯3'),(1215,925,325,'g♯3')]:
    k=rect(x,y,w,18,'pink','focus' if t=='g♯3' else 'tertiary',par='piano-roll');txt(x+3,y+1,w-6,t,10,par=k)
line(1212,844,325,0,'focus',style=dict(lineWidth=3),par='piano-roll')
txt(805,854,27,'c4',9,par='piano-roll');txt(805,1067,27,'c3',9,par='piano-roll')
# Row backgrounds intentionally underlay piano keys, loop brace, and notes.
rows=[n for n in nodes if n.get('parentId')=='piano-roll' and n['type']=='container' and n['bounds']['height']==8.5]
for row in rows:
    members={'piano-roll'}
    for n in nodes:
        if n.get('parentId') in members:members.add(n['id'])
    row['allowOverlapWith']=[id for id in members if id!=row['id']]
# Preserve absolute geometry while making panel membership explicit for Frame review.
by_id={n['id']:n for n in nodes}
for n in nodes:
    if n['id'].startswith('session-') and n['type']=='button':n['parentId']='shell'
    if n['type']=='line':continue
    x,y=positions[n['id']];b=n['bounds']
    candidates=[]
    for area in ['browser-nav','timeline','verb','eq']:
        if n['id']==area:continue
        a=by_id[area];ax,ay=positions[area];ab=a['bounds']
        if n.get('parentId')==a.get('parentId') and ax<=x and ay<=y and x+b['width']<=ax+ab['width'] and y+b['height']<=ay+ab['height']:candidates.append(a)
    if not candidates and n['type']=='text':
        for a in nodes:
            if a['id']==n['id']:break
            if a['type']!='container' or a.get('text') or a.get('parentId')!=n.get('parentId'):continue
            ax,ay=positions[a['id']];ab=a['bounds']
            if ax<=x and ay<=y and x+b['width']<=ax+ab['width'] and y+b['height']<=ay+ab['height']:candidates.append(a)
    if candidates:
        a=min(candidates,key=lambda a:a['bounds']['width']*a['bounds']['height']);ax,ay=positions[a['id']]
        n['parentId']=a['id'];b['x']=x-ax;b['y']=y-ay
for n in nodes:
    if n['type']!='text':continue
    x,y=positions[n['id']];b=n['bounds']
    for a in nodes:
        if a['id']==n['id']:break
        if a['type']!='container' or a.get('text') or a.get('parentId')!=n.get('parentId'):continue
        ax,ay=positions[a['id']];ab=a['bounds']
        if ax<=x and ay<=y and x+b['width']<=ax+ab['width'] and y+b['height']<=ay+ab['height']:
            n['parentId']=a['id'];b['x']=x-ax;b['y']=y-ay;break
# Scrollbar thumb intentionally overlays its trough.
scroll=[n for n in nodes if positions[n['id']]==(437,723)][0]
scroll['allowOverlapWith']=[n['id'] for n in nodes if positions[n['id']]==(710,726)]
# Time selection is an explicit visual overlay over three adjacent track lanes.
parent='arrangement'
selection=rect(662,227,224,202,'#00e8df18','#00e8df66',id='time-range',par='timeline')
nodes[-1]['allowOverlapWith']=[n['id'] for n in nodes[:-1] if n['id'] not in ['shell','arrangement','timeline'] and positions[n['id']][0]<886 and positions[n['id']][0]+n['bounds']['width']>662 and positions[n['id']][1]<429 and positions[n['id']][1]+n['bounds']['height']>227]
line(662,227,0,202,'focus',par=selection)
line(886,227,0,202,'focus',par=selection)
rect(662,106,224,4,'focus','none',id='time-range-ruler',par='timeline')
nodes[-1]['allowOverlapWith']=[n['id'] for n in nodes[:-1] if n['id'] not in ['shell','arrangement','timeline'] and positions[n['id']][0]<886 and positions[n['id']][0]+max(1,n['bounds']['width'])>662 and positions[n['id']][1]<110 and positions[n['id']][1]+max(1,n['bounds']['height'])>106]
parent=None
# Four states retain the existing transport/view prototype contract.
parent=None
states={}
for state in ['default','playing','session','session-playing']:
    session=state.startswith('session');playing=state.endswith('playing')
    states[state]={'arrangement':dict(visible=not session),'session-view':dict(visible=session),'play':dict(visible=not playing),'pause':dict(visible=playing)}
states['time-selection']={**states['default']}
for state in states:
    states[state]['time-range']=dict(visible=state=='time-selection')
    states[state]['time-range-ruler']=dict(visible=state=='time-selection')
# Meter paint is only visible in simulated playback states.
for state in states:
    for element in meter_paint:states[state][element]=dict(visible=state.endswith('playing'))
# Dedicated session transport controls point to session states.
for id,x,text,target,label in [('session-play',825,'▶','session-playing','play session · space'),('session-pause',825,'‖','session','pause session · space'),('session-stop',850,'■','session','stop session · escape')]:
    parent='shell';action(id,x,30,22,text,target,label);parent=None
    for s in states:states[s][id]=dict(visible=s.startswith('session') and ((s.endswith('playing'))==(id=='session-pause') if id!='session-stop' else True))
for s in states:
    if s.startswith('session'):
        for id in ['play','pause','stop']:states[s][id]=dict(visible=False)
    states[s]['session-tab']=dict(style=dict(fill=C['amber'] if s.startswith('session') else C['dark'],color='#20282c' if s.startswith('session') else C['muted']))
    states[s]['arrange-tab']=dict(style=dict(fill=C['dark'] if s.startswith('session') else C['amber'],color=C['muted'] if s.startswith('session') else '#20282c'))
# Shared clip-color menu: swatches are a visual specification, not edit commands.
parent='shell'
action('color-menu',1865,1097,22,'▦','clip-colors','custom clip color picker')
rect(863,248,526,410,THEME['overlay'],THEME['separator'],id='color-panel')
parent='color-panel'
txt(879,262,423,'custom clip color',13)
action('color-close',1352,260,22,'×','session','close color picker')
txt(879,287,492,'any color · your swatches · up to 64',10,'muted')
# Editable Frame paint cells represent the saturation/value plane and hue strip.
hue=colorsys.rgb_to_hsv(240/255,100/255,73/255)[0]
for row in range(20):
    for col in range(32):
        rgb=colorsys.hsv_to_rgb(hue,col/31,1-row/19)
        color='#'+''.join(f'{round(c*255):02x}' for c in rgb)
        rect(879+col*8,316+row*8,8,8,color,'none')
cursor=rect(1013,340,8,8,'none','#eef1eb',style=dict(radius=4))
nodes[-1]['allowOverlapWith']=[n['id'] for n in nodes[:-1] if n.get('parentId')=='color-panel' and n['bounds']['x']<158 and n['bounds']['x']+n['bounds']['width']>150 and n['bounds']['y']<100 and n['bounds']['y']+n['bounds']['height']>92]
for col in range(64):
    rgb=colorsys.hsv_to_rgb(col/64,1,1)
    rect(879+col*4,486,4,12,'#'+''.join(f'{round(c*255):02x}' for c in rgb),'none')
rect(889,484,4,16,'none','#eef1eb',allowOverlapWith=[n['id'] for n in nodes if n.get('parentId')=='color-panel' and n['bounds']['x']<30 and n['bounds']['x']+n['bounds']['width']>26 and n['bounds']['y']<252 and n['bounds']['y']+n['bounds']['height']>236])
rect(1154,316,214,58,THEME['orange'],'grid')
txt(1154,389,210,'hex',10,'muted');box(1154,408,214,'#f06449',size=12,h=24)
txt(1154,445,210,'rgb',10,'muted')
for col,label in enumerate(['240','100','73']):box(1154+col*73,464,68,label,size=11,h=24)
box(1154,506,214,'+ save swatch','amber',11,h=24,color='dark')
txt(879,550,492,'saved swatches / 8 of 64',11)
for col,color in enumerate(PICKER['savedSwatches']):
    rect(879+col*61,574,54,32,color,'grid',id='swatch-saved-'+str(col))
txt(879,626,492,'personal colors · independent of the interface theme',9,'muted')
parent=None
for state in states:states[state]['color-panel']=dict(visible=False)
states['clip-colors']={**states['session'],'color-panel':dict(visible=True)}
# Declare only actual popup overlaps with underlying Session descendants.
session_members={'session-view'}
for n in nodes:
    if n.get('parentId') in session_members:session_members.add(n['id'])
for n in nodes:
    if n['id']=='color-panel' or n.get('parentId')=='color-panel' or n.get('parentId','').startswith('swatch-'):
        x,y=positions[n['id']];b=n['bounds'];overlaps=[]
        for other in nodes:
            if other['id'] not in session_members:continue
            ox,oy=positions[other['id']];ob=other['bounds']
            if x<ox+ob['width'] and x+b['width']>ox and y<oy+ob['height'] and y+b['height']>oy:overlaps.append(other['id'])
        if overlaps:n['allowOverlapWith']=n.get('allowOverlapWith',[])+overlaps
# Strong workspace outlines use the same quiet separator as their neighbors.
for n in nodes:
    if n['style'].get('stroke') in [C['muted']]:n['style']['stroke']=THEME['separator']
    if n['type']=='text' and n.get('text') in ['f♯3','g♯3']:
        n['style']['color']=clip_ink(C['pink'])
doc=dict(version=1,id='hex-yield-workstation',name='hex / orbital',revisionId='hex-live-r68',width=W,height=H,background=C['bg'],tokens=dict(colors=dict(background=C['bg'],surface=C['panel'],border=C['grid'],text=C['text'],muted=C['muted'],accent=C['amber']),typography=dict(family='Ioskeley Mono',size=11)),states=states,elements=nodes)
(OUT/'hex-live-r68.json').write_text(json.dumps(doc,indent=2)+'\n')
print(f'{len(nodes)} editable elements; {W} × {H}')

# Main UI tokens and independently customizable clip colors.
master=dict(version=1,id='hex-theme',name='orbital',anchors=['violet','blue','sea_glass','orange'],theme=THEME,conceptColors=CONCEPT_COLORS,clipPicker=PICKER)
(OUT/'theme-palette.json').write_text(json.dumps(master,indent=2)+'\n')
# Frame palette board makes the overall relationships reviewable before the UI.
nodes=[];positions={};serial=0;parent=None
rect(0,0,1080,850,'bg','none',id='palette-board')
parent='palette-board'
txt(32,26,1000,'hex / orbital',20)
txt(32,58,1000,'charcoal surfaces · gray + bright orange · vivid secondary green · soft violet tertiary · quiet blue · deliberate signals',11,'muted')
roles=['background','surface','well','overlay','separator','text','violet','blue','sea_glass','orange']
for i,role in enumerate(roles):
    x=32+(i%5)*204;y=98+(i//5)*88
    color=THEME[role];sw=rect(x,y,184,42,color,'grid')
    txt(x,y+46,184,role.replace('_',' '),11)
    txt(x,y+63,184,color,10,'muted')
txt(32,301,1000,'theme in use / one harmonious clip fill, clear accent roles',11,'muted')
for col,(role,key,label) in enumerate([('violet','purple','tertiary details'),('blue','blue','parameters'),('sea_glass','cyan','secondary highlight'),('orange','coral','primary action')]):
    x=32+col*254
    card=rect(x,337,243,119,'surface','separator')
    paint=rect(x+12,353,219,61,CONCEPT_COLORS[key],'separator',par=card)
    for j in range(26):
        amp=5+15*abs(math.sin(j*1.7))
        line(x+20+j*8,383-amp/2,0,amp,clip_ink(CONCEPT_COLORS[key]),par=paint)
    rect(x+12,430,18,8,THEME[role],'none',par=card)
    txt(x+39,425,190,label,10,'muted',par=card)
txt(32,493,1000,'clip colors belong to the user',13)
txt(32,523,1000,'free color picker · personal saved swatches · maximum 64 · no fixed hue families',11,'muted')
for col,color in enumerate(PICKER['savedSwatches']):rect(32+col*127,559,116,35,color,'grid')
txt(32,640,1000,'semantic signals / reserved for specific interface states',11,'muted')
for col,role in enumerate(['success','warning','error']):
    x=32+col*340; color=THEME[role]
    rect(x,672,312,46,color,'grid')
    txt(x,729,312,role+' / '+color,11)
    txt(x,752,312,{'success':'completed / healthy','warning':'attention / recoverable','error':'failed / action required'}[role],10,'muted')
txt(32,819,1000,'orange = action  ·  green = secondary focus  ·  violet = tertiary editing  ·  blue = parameters',11,'muted')
palette_doc=dict(version=1,id='hex-theme-board',name='hex / unified theme palette',revisionId='hex-theme-r44',width=1080,height=850,background=C['bg'],tokens=doc['tokens'],states={'default':{}},elements=nodes)
(OUT/'theme-palette.frame.json').write_text(json.dumps(palette_doc,indent=2)+'\n')
