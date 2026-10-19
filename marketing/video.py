import subprocess, math, sys, os, io
import numpy as np
from PIL import Image, ImageDraw, ImageFilter
import base as M
from base import font,tlayer,rmask,solid,shadow_rr,card_layer,put,seg,clamp,lerp,eo3,eo5,ei3,eio3,rot,pen_layer,notebook_base,FF,S,OUT,FD,FM,FSB,SERIF,SERIFI,INK,INK2,ACC,CARD,HL,GREEN,BLUE,AMBER,RED,PAPER
W,H=1080,1920;FPS=60
BGI=M.BG.crop((210,190,210+W,190+H))
def eq(x): x=clamp(x); return 1-(1-x)**5           # entrance: ease-out quint
def ex(x): x=clamp(x); return x**3                  # exit: ease-in cubic
def cam(x): x=clamp(x); return x*x*(3-2*x) if False else (4*x**3 if x<.5 else 1-(-2*x+2)**3/2)

# ---------- mask-reveal type: each line rises from behind an invisible edge
STY={'xl':(FD,140,INK),'l':(FD,116,INK),'la':(FD,116,ACC),'m':(FM,46,INK2),'s':(FM,40,INK2),'w':(SERIF,250,INK),'wi':(SERIFI,104,ACC)}
def mline(cv,t,t0,t1,text,style,cy):
    if t<t0 or (t1 is not None and t>t1+0.45):return
    fp,sz,col=STY[style];lay=tlayer(text,fp,sz,col);w,h=lay.size
    p=eq(seg(t,t0,t0+0.75));q=ex(seg(t,t1,t1+0.42)) if t1 is not None else 0
    off=int((1-p)*h*0.95-q*h*0.95)
    win=Image.new('RGBA',(w,h),col+(0,));win.paste(lay,(0,off),lay)
    put(cv,win,W/2,cy)
def mblock(cv,t,t0,t1,lines,y,gap=1.08,stag=0.09):
    for i,(st,tx) in enumerate(lines):
        fp,sz,col=STY[st]
        mline(cv,t,t0+i*stag,None if t1 is None else t1+i*0.04,tx,st,y+sz*0.62)
        y+=int(sz*gap)+(14 if st in('m','s') else 0)

# ---------- screen view (shared element: phone <-> close-up card)
PW=760
def FULL(push=0.0):
    w=PW*(1+0.05*push);h=w*2344/1080;return dict(x=(W-w)/2,y=470-30*push,w=w,h=h,sx=0,sy=0,sw=1080,sh=2344,b=1.0,r=0.095*w)
def MACRO(fy):
    w,h=1000,1350;sw=1080;sh=sw*h/w;sy=clamp(fy-sh/2,0,2344-sh)
    return dict(x=40,y=480,w=w,h=h,sx=0,sy=sy,sw=sw,sh=sh,b=0.0,r=46)
def mix(a,b,k): return {key:lerp(a[key],b[key],k) for key in a}
def draw_view(cv,src,v,alpha=1.0,shadow=1.0):
    x,y,w,h=v['x'],v['y'],int(v['w']),int(v['h']);r=int(v['r'])
    crop=src.crop((int(v['sx']),int(v['sy']),int(v['sx']+v['sw']),int(v['sy']+v['sh']))).resize((w,h),Image.BILINEAR)
    bt=int(16*v['b'])
    if shadow>0.02:
        sh=shadow_rr(w+2*bt,h+2*bt,r+bt,blur=40,off=34,alpha=int(80*shadow),pad=100)
        cv.paste(solid(sh.width,sh.height,(70,48,22)),(int(x)-bt-100,int(y)-bt-100),sh)
    if bt>0:cv.paste(solid(w+2*bt,h+2*bt,(20,20,22)),(int(x)-bt,int(y)-bt),rmask(w+2*bt,h+2*bt,r+bt))
    cv.paste(crop,(int(x),int(y)),rmask(w,h,r))

def tilt(img,p):
    """perspective entrance: phone leans back and straightens"""
    if p>=0.999:return img
    w,h=img.size;k=0.16*(1-p);dh=0.10*(1-p)
    # destination quad (top edge narrower and lower)
    dst=[(w*k,h*dh),(w*(1-k),h*dh),(w,h),(0,h)];src=[(0,0),(w,0),(w,h),(0,h)]
    A=[];B=[]
    for (X,Y),(u,v) in zip(dst,src):
        A.append([X,Y,1,0,0,0,-u*X,-u*Y]);B.append(u);A.append([0,0,0,X,Y,1,-v*X,-v*Y]);B.append(v)
    c=np.linalg.solve(np.array(A,float),np.array(B,float))
    return img.transform((w,h),Image.PERSPECTIVE,tuple(c),Image.BICUBIC)

# ---------- shots
class Shot:
    def __init__(s,t0,kind,f,ss,speed,dur):s.t0=t0;s.kind=kind;s.f=f;s.ss=ss;s.speed=speed;s.dur=dur;s.t1=t0+dur;s.to=ss+dur*speed
SH=[];T=8.4
def add(kind,f,ss,speed,dur):
    global T;s=Shot(T,kind,f,ss,speed,dur);SH.append(s);T+=dur;return s
a=add('clip','m1.mp4',12,1.8,4.6)
b=add('clip','m1.mp4',51,1.5,4.3)
b2=add('clip','m1.mp4',58,2.0,4.0)
c=add('clip','m1.mp4',74,2.2,4.5)
d=add('clip','m2.mp4',50.6,1.1,5.0)
e=add('clip','m3.mp4',3.5,1.5,6.3)
f=add('still','a2.png',0,1,3.2)
PH_OUT=f.t1
P0=PH_OUT+0.4;N0=P0+4.6;E0=N0+5.8;TOTAL=E0+4.6
KEYS=[(8.4,FULL(0)),(a.t1,FULL(0.6)),(b.t0+0.8,MACRO(1930)),(b2.t0,MACRO(1930)),(b2.t0+0.7,MACRO(1760)),(c.t0,MACRO(1760)),(c.t0+0.7,MACRO(1690)),(d.t0,MACRO(1690)),(d.t0+0.9,FULL(0)),(e.t0,FULL(0.5)),(e.t1-0.6,FULL(0.9)),(f.t0+0.3,MACRO(1150)),(PH_OUT,MACRO(1150))]
def view_at(t):
    if t<=KEYS[0][0]:return KEYS[0][1]
    for (t0,v0),(t1,v1) in zip(KEYS,KEYS[1:]):
        if t0<=t<=t1:
            same=all(abs(v0[k]-v1[k])<1e-6 for k in v0) or (v0['b']==v1['b']==1.0)
            k=seg(t,t0,t1);return mix(v0,v1,k if v0['b']==v1['b']==1.0 and abs(v0['w']-v1['w'])<60 else cam(k))
    return KEYS[-1][1]
def shot_at(t):
    for s in SH:
        if s.t0<=t<s.t1:return s
class Clip:
    def __init__(s,sh):
        s.proc=subprocess.Popen([FF,'-v','error','-ss',str(sh.ss),'-t',str(sh.to-sh.ss+0.5),'-i',S+'/'+sh.f,'-vf',f'setpts=(PTS-STARTPTS)/{sh.speed},fps={FPS}','-f','rawvideo','-pix_fmt','rgb24','-'],stdout=subprocess.PIPE)
        s.last=Image.new('RGB',(1080,2344),(10,10,20))
    def next(s):
        n=1080*2344*3;b=s.proc.stdout.read(n)
        if len(b)==n:s.last=Image.frombytes('RGB',(1080,2344),b)
        return s.last
    def close(s):
        try:s.proc.kill()
        except Exception:pass

# ---------- scenes
CASCADE=[('Standup.',0.0),('Sprint planning.',0.5),('Design review.',1.0),('Another sync.',1.5)]
def scene_hook(cv,t):
    for i,(tx,t0) in enumerate(CASCADE):
        t1=CASCADE[i+1][1]-0.12 if i+1<len(CASCADE) else 2.25
        mline(cv,t,t0,t1,tx,'xl',880)
    nin=eq(seg(t,2.15,2.95));nout=ex(seg(t,6.3,6.9))
    if nin>0 and nout<1:
        pw=seg(t,2.4,4.5);er=eq(seg(t,5.0,6.2))
        lay,_=notebook_base(int(pw*100));lay=lay.copy()
        if er>0:
            clean,_=notebook_base(0);top=int(lerp(900,150,er))
            band=clean.crop((80,top,720,900));m=Image.new('L',band.size,255)
            g=ImageDraw.Draw(m)
            for i in range(60):g.line([(0,i),(band.width,i)],fill=int(255*i/60))
            lay.paste(band,(80,top),m)
        if pw<1:
            _,head=notebook_base(int(pw*100));pen=rot(pen_layer(),-28);lay.paste(pen,(int(head[0]-10),int(head[1]-pen.height+22)),pen)
        lay=rot(lay,-3)
        put(cv,lay,540,1180+(1-nin)*700+nout*900)
    mblock(cv,t,2.45,4.55,[('l','Notes pile up.')],150)
    mblock(cv,t,4.8,6.4,[('l','Context'),('la','slips away.')],150)
    mline(cv,t,6.95,8.25,'Meet',"m",700)
    mline(cv,t,7.1,8.3,'Marginalia','w',860)
CAP=[(a,'It listens.','Live captions as people talk.'),(b,'Tag what matters.','Note, Action, Decision, Question.'),
     (b2,'The AI tidies it.','Using what was just said.'),(c,'It remembers.','Old meetings chime in.'),
     (d,'Notes, written','for you.'),(e,'Ask anything.','What did we decide?'),(f,'With sources.','Down to the minute.')]
def scene_phone(cv,t,src):
    if src is not None and t>=8.25 and t<PH_OUT+0.8:
        v=view_at(t);pin=eq(seg(t,8.3,9.4));pout=ex(seg(t,PH_OUT,PH_OUT+0.6))
        v=dict(v);v['y']+= (1-pin)*900+pout*1500
        if pin<0.999:
            # render phone onto its own layer for the tilt
            w,h=int(v['w']),int(v['h']);lay=Image.new('RGBA',(w+40,h+40),(0,0,0,0))
            tmp=Image.new('RGB',lay.size,(0,0,0));vv=dict(v);vv['x']=20;vv['y']=20
            draw_view(tmp,src,vv,shadow=0);mask=Image.new('L',lay.size,0)
            mask.paste(rmask(w+32,h+32,int(v['r'])+16),(4,4));lay=Image.merge('RGBA',(*tmp.split(),mask))
            lay=tilt(lay,pin)
            sh=shadow_rr(w,h,int(v['r']),blur=40,off=34,alpha=int(80*pin),pad=100);cv.paste(solid(sh.width,sh.height,(70,48,22)),(int(v['x'])-100,int(v['y'])-100),sh)
            cv.paste(lay,(int(v['x'])-20,int(v['y'])-20),lay)
        else:draw_view(cv,src,v)
    for s,h1,h2 in CAP:
        t0=s.t0+(0.35 if s is a else 0.15);t1=s.t1-0.35
        mblock(cv,t,t0,t1,[('l',h1),('m',h2)] if s is not d else [('l',h1),('l',h2)],140)
def lock(cv,cx,cy,p):
    if p<=0:return
    lay=Image.new('RGBA',(220,260),(0,0,0,0));d=ImageDraw.Draw(lay)
    d.arc([45,10,175,150],180,360,fill=INK+(255,),width=20);d.rectangle([45,78,65,120],fill=INK+(255,));d.rectangle([155,78,175,120],fill=INK+(255,))
    body=Image.new('RGBA',(200,150),ACC+(255,));body.putalpha(rmask(200,150,34));lay.paste(body,(10,104),body)
    d.ellipse([96,150,124,178],fill=CARD+(255,));d.rectangle([104,170,116,206],fill=CARD+(255,))
    put(cv,lay,cx,cy+(1-p)*40,alpha=eo3(p),sx=lerp(0.92,1,p))
def scene_private(cv,t):
    u=t-P0
    lock(cv,540,560,eq(seg(u,0.0,0.7)))
    mblock(cv,t,P0+0.3,P0+4.2,[('xl','All on'),('xl','your phone.')],760)
    mline(cv,t,P0+1.3,P0+4.25,'No account. No cloud.','m',1160)
    mline(cv,t,P0+1.6,P0+4.3,'Your meetings stay in the room.','m',1240)
def scene_node(cv,t):
    u=t-N0
    mblock(cv,t,N0+0.1,N0+2.3,[('l','Need it faster?')],150)
    mblock(cv,t,N0+2.55,N0+5.3,[('l','Borrow your'),('la','own PC.')],150)
    q=ex(seg(u,5.2,5.7));pin=eq(seg(u,0.3,1.1))
    if pin<=0 or q>=1:return
    yo=(1-pin)*120+q*80;al=eo3(pin)*(1-q)
    # phone and laptop as simple shapes
    still=M.getstill('a5.png');pw=250;ph=int(pw*2344/1080);px,py=250,1060+yo
    ph_l=Image.new('RGBA',(pw+14,ph+14),(0,0,0,0));ph_l.paste(solid(pw+14,ph+14,(20,20,22)),(0,0),rmask(pw+14,ph+14,34));ph_l.paste(still.resize((pw,ph),Image.BILINEAR),(7,7),rmask(pw,ph,28))
    put(cv,ph_l,px,py,alpha=al)
    lx,ly=790,1040+yo;lw,lh=420,270
    lap=Image.new('RGBA',(520,320),(0,0,0,0));lap.paste(solid(lw,lh,(28,27,32)),(50,0),rmask(lw,lh,24));lap.paste(solid(lw-26,lh-26,(40,40,60)),(63,13),rmask(lw-26,lh-26,16))
    lap.paste(solid(500,26,(196,190,178)),(10,lh),rmask(500,26,13))
    busy=1.0<u<4.0
    dd=ImageDraw.Draw(lap);dd.ellipse([90,120,118,148],fill=BLUE if busy else GREEN);l=tlayer('Working' if busy else 'Ready',FSB,42,(240,240,250));lap.paste(l,(132,104),l)
    put(cv,lap,lx,ly,alpha=al)
    put(cv,tlayer('your phone',FM,36,INK2),px,py+ph/2+50,alpha=al);put(cv,tlayer('your PC',FM,36,INK2),lx,ly+210,alpha=al)
    d=ImageDraw.Draw(cv)
    for i in range(11):
        x=px+150+i*(lx-260-px-150)/10;ph_=(u*1.6-i/10)%1.0
        r=4+5*max(0,1-abs(ph_-0.5)*6) if 1.0<u<4.0 else 4
        if al>0.5:d.ellipse([x-r,ly-r,x+r,ly+r],fill=(170,160,236) if r>4.5 else (190,182,166))
    if u>2.9:
        k=eq(seg(u,2.9,3.6))
        put(cv,card_layer('It keeps nothing',46,FD,44,26,dot=GREEN),540,1510+(1-k)*30,alpha=eo3(k)*(1-q))
    if u>3.4:
        k=eq(seg(u,3.4,4.1))
        put(cv,tlayer('A 19 minute talk, speakers sorted in 23 seconds.',FM,38,INK2),540,1640+(1-k)*20,alpha=eo3(k)*(1-q))
def scene_end(cv,t):
    mline(cv,t,E0+0.1,None,'Marginalia','w',820)
    mline(cv,t,E0+0.55,None,'Put the pen down.','wi',1000)
    u=t-E0;k=eq(seg(u,1.3,2.0))
    put(cv,tlayer('Meeting notes for Android. Private by design.',FM,38,INK2),540,1740+(1-k)*20,alpha=eo3(k))
def render(t,src):
    cv=BGI.copy()
    if t<8.6:scene_hook(cv,t)
    scene_phone(cv,t,src)
    if P0-0.05<=t<N0+0.5:scene_private(cv,t)
    if N0<=t<E0+0.2:scene_node(cv,t)
    if t>=E0:scene_end(cv,t)
    return cv
def frame_src(t):
    s=shot_at(t)
    if s is None:return M.getstill('a2.png') if t>=SH[-1].t1 else None
    if s.kind=='still':return M.getstill(s.f)
    return M.clip_at(s.f,s.ss,t-s.t0,s.speed)
def test(ts):
    for t in ts:render(t,frame_src(t) if t>=8.25 else None).save(f'{OUT}/frames/v_{int(t*10):04d}.png')
def full():
    tmp=S+'/v3.mp4'
    enc=subprocess.Popen([FF,'-y','-v','error','-f','rawvideo','-pix_fmt','rgb24','-s',f'{W}x{H}','-r',str(FPS),'-i','-','-c:v','libx264','-pix_fmt','yuv420p','-crf','18','-preset','medium','-movflags','+faststart',tmp],stdin=subprocess.PIPE)
    n=int(TOTAL*FPS);cur=None;clip=None
    for i in range(n):
        t=i/FPS;s=shot_at(t);src=None
        if s is not cur:
            if clip:clip.close();clip=None
            cur=s
            if s and s.kind=='clip':clip=Clip(s)
        if s:src=clip.next() if clip else M.getstill(s.f)
        elif t>=SH[-1].t1:src=M.getstill('a2.png')
        elif t>=8.25:src=None
        if s is None and SH[0].t0-0.2<=t<SH[0].t0:src=M.clip_at(SH[0].f,SH[0].ss,0,1)
        enc.stdin.write(render(t,src).tobytes())
        if i%300==0:print(i,n,flush=True)
    enc.stdin.close();enc.wait();os.replace(tmp,OUT+'/marginalia-demo.mp4');print('done',TOTAL)
if __name__=='__main__':
    if sys.argv[1]=='test':test([float(x) for x in sys.argv[2:]])
    else:full()
