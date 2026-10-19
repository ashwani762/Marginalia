import subprocess, math, sys, os, random, io, wave
from functools import lru_cache
from PIL import Image, ImageDraw, ImageFont, ImageFilter
import numpy as np

HERE=os.path.dirname(os.path.abspath(__file__))
REPO=os.path.dirname(HERE)
S=os.environ.get('MARGINALIA_RECORDINGS',os.path.join(HERE,'recordings'))  # m1.mp4, m2.mp4, m3.mp4, a2.png, a5.png, n1.png, n3.png, rec3.png
OUT=os.environ.get('MARGINALIA_OUT',os.path.join(HERE,'out'))
APPFONT=os.path.join(REPO,'app/src/main/res/font')
try:
    import imageio_ffmpeg;FF=imageio_ffmpeg.get_ffmpeg_exe()
except ImportError:
    FF='ffmpeg'
os.makedirs(OUT,exist_ok=True);os.makedirs(os.path.join(OUT,'frames'),exist_ok=True)
W,H=1080,1920; FPS=30
INK=(28,27,24); INK2=(112,106,94); ACC=(98,80,230); CARD=(255,253,248); HL=(218,210,255)
GREEN=(40,170,100); AMBER=(238,162,30); RED=(222,64,64); BLUE=(54,128,240); PAPER=(255,252,243)
FD='/usr/share/fonts/opentype/inter/InterDisplay-Bold.otf'
FM='/usr/share/fonts/opentype/inter/Inter-Medium.otf'
FSB='/usr/share/fonts/opentype/inter/Inter-SemiBold.otf'
SERIF=APPFONT+'/instrument_serif.ttf'; SERIFI=APPFONT+'/instrument_serif_italic.ttf'
SFX=[]  # (time, kind, gain)
def sfx(t,kind,g=1.0): SFX.append((t,kind,g))

@lru_cache(None)
def font(p,s): return ImageFont.truetype(p,int(s))
def clamp(x,a=0.0,b=1.0): return max(a,min(b,x))
def lerp(a,b,t): return a+(b-a)*t
def seg(t,a,b): return clamp((t-a)/(b-a))
def eo3(x): x=clamp(x); return 1-(1-x)**3
def eo5(x): x=clamp(x); return 1-(1-x)**5
def ei3(x): x=clamp(x); return x**3
def eio3(x): x=clamp(x); return 4*x**3 if x<.5 else 1-(-2*x+2)**3/2
def eob(x,s=1.9): x=clamp(x); return 1+(s+1)*(x-1)**3+s*(x-1)**2
def bump(p): p=clamp(p); return math.sin(math.pi*p)*(1-p)   # squash pulse that dies out

# ---------- background (ambient layer)
def make_bg():
    BW,BH=1500,2300
    im=Image.new('RGB',(BW,BH));d=ImageDraw.Draw(im)
    for y in range(BH):
        t=y/BH;d.line([(0,y),(BW,y)],fill=tuple(int(a*(1-t)+b*t) for a,b in zip((248,242,230),(240,232,216))))
    for box,col,a in [([100,200,800,900],(252,208,176),170),([700,1300,1400,2000],(214,204,252),170),([500,800,1000,1250],(255,234,192),120)]:
        m=Image.new('L',(BW,BH),0);ImageDraw.Draw(m).ellipse(box,fill=a);m=m.filter(ImageFilter.GaussianBlur(150))
        im.paste(Image.new('RGB',(BW,BH),col),(0,0),m)
    return im
BG=make_bg()
random.seed(3)
DOTS=[(random.uniform(0,W),random.uniform(0,H),random.uniform(5,13),random.uniform(14,34),random.choice([(240,214,190),(222,214,248),(246,226,196)])) for _ in range(14)]
def bg_canvas(t,push=0.0):
    dx=int(210+70*math.sin(t*0.35));dy=int(190+90*math.cos(t*0.27)+push*70)
    cv=BG.crop((dx,dy,dx+W,dy+H));d=ImageDraw.Draw(cv)
    for x,y,r,v,c in DOTS:
        yy=(y-t*v)%(H+60)-30;xx=x+14*math.sin(t*0.8+x)
        d.ellipse([xx-r,yy-r,xx+r,yy+r],fill=c)
    return cv

# ---------- primitives
@lru_cache(512)
def rmask(w,h,r):
    r=max(2,min(r,w//2-1,h//2-1));m=Image.new('L',(w,h),0);d=ImageDraw.Draw(m)
    d.rectangle([r,0,w-r-1,h-1],fill=255);d.rectangle([0,r,w-1,h-r-1],fill=255)
    c=Image.new('L',(4*r,4*r),0);ImageDraw.Draw(c).ellipse([0,0,4*r-1,4*r-1],fill=255);c=c.resize((2*r,2*r),Image.LANCZOS)
    for (qx,qy,px,py) in [(0,0,0,0),(r,0,w-r,0),(0,r,0,h-r),(r,r,w-r,h-r)]:
        m.paste(c.crop((qx,qy,qx+r,qy+r)),(px,py))
    return m
@lru_cache(None)
def solid(w,h,c): return Image.new('RGB',(w,h),c)
@lru_cache(4096)
def tlayer(text,fp,size,color):
    f=font(fp,size);w=int(f.getlength(text))+10;h=int(size*1.5)
    im=Image.new('RGBA',(w,h),color+(0,));ImageDraw.Draw(im).text((4,int(size*0.12)),text,font=f,fill=color+(255,));return im
def put(cv,layer,cx,cy,alpha=1.0,sx=1.0,sy=None):
    sy=sx if sy is None else sy
    if alpha<=0.01 or sx<=0.01:return
    if abs(sx-1)>0.002 or abs(sy-1)>0.002:layer=layer.resize((max(1,int(layer.width*sx)),max(1,int(layer.height*sy))),Image.BILINEAR)
    if alpha<0.995:
        layer=layer.copy();layer.putalpha(layer.getchannel('A').point(lambda v:int(v*alpha)))
    cv.paste(layer,(int(cx-layer.width/2),int(cy-layer.height/2)),layer)
def shadow_rr(w,h,r,blur=26,off=14,alpha=70,pad=70):
    s=Image.new('L',(w+2*pad,h+2*pad),0);ImageDraw.Draw(s).rounded_rectangle([pad,pad+off,pad+w,pad+off+h],radius=r,fill=alpha)
    return s.filter(ImageFilter.GaussianBlur(blur))
@lru_cache(512)
def card_layer(text,size=50,fp=FD,padx=46,pady=30,color=INK,fill=CARD,dot=None):
    f=font(fp,size);tw=int(f.getlength(text));dotw=(size*0.62+22) if dot else 0
    w=int(tw+2*padx+dotw);h=int(size*1.25+2*pady);pad=70
    lay=Image.new('RGBA',(w+2*pad,h+2*pad),(0,0,0,0))
    sh=shadow_rr(w,h,h//2,pad=pad);lay.paste((60,40,20,255),(0,0),sh)
    lay.paste(Image.new('RGB',(w,h),fill),(pad,pad),rmask(w,h,h//2))
    d=ImageDraw.Draw(lay);x=pad+padx
    if dot:
        rr=size*0.31;cy=pad+h/2;d.ellipse([x,cy-rr,x+2*rr,cy+rr],fill=dot);x+=2*rr+22
    d.text((x,pad+pady+int(size*0.04)),text,font=f,fill=color)
    return lay
def pop(cv,layer,cx,cy,t,t0,t1=None,float_amp=8,ph=0.0):
    """Playful entrance: wind-up scale from 0.5, ease-out-back (about 20% overshoot), squash on landing, ambient float, quick ease-in exit."""
    if t<t0 or (t1 is not None and t>t1+0.25):return
    p=seg(t,t0,t0+0.34);a=eo3(seg(t,t0,t0+0.15));s=lerp(0.5,1.0,eob(p,2.4));b=bump(p)
    sx,sy=s*(1+0.10*b),s*(1-0.10*b);yo=(1-eo5(p))*36
    if t1 is not None:
        q=ei3(seg(t,t1,t1+0.22));a*=1-q;sx*=1-0.25*q;sy*=1-0.25*q;yo-=24*q
    yo+=float_amp*math.sin(t*1.5+ph)*eo3(seg(t,t0+0.4,t0+0.9))
    put(cv,layer,cx,cy+yo,a,sx,sy)

# ---------- headline words (kinetic type)
STY={'h':(FD,96,INK),'hs':(SERIF,150,INK),'sub':(FM,42,INK2),'acc':(FD,96,ACC),'red':(FD,96,RED),'big':(FD,118,INK)}
def headline(cv,t,t0,t1,lines,y=110,stag=0.045,snd=True):
    if t<t0 or t>t1+0.5:return
    idx=0
    for style,text in lines:
        fp,sz,col=STY[style];f=font(fp,sz);sp=f.getlength(' ')
        words=text.split();total=sum(f.getlength(w) for w in words)+sp*(len(words)-1);x=(W-total)/2
        for w in words:
            ts=t0+idx*stag;p=seg(t,ts,ts+0.34);q=ei3(seg(t,t1+idx*0.02,t1+idx*0.02+0.22));idx+=1
            wl=f.getlength(w);lay=tlayer(w,fp,sz,col)
            if style in('acc','red') and p>0 and q<0.6:
                hw=int(wl*eo3(seg(t,ts+0.2,ts+0.55)))
                if hw>4:
                    hl=Image.new('RGBA',(hw,int(sz*0.42)),(HL if style=='acc' else (255,214,210))+(255,))
                    hl.putalpha(rmask(hw,int(sz*0.42),12));cv.paste(hl,(int(x),int(y+sz*0.78)),hl)
            if p>0 and q<1:
                b=bump(p);sc=lerp(0.8,1.0,eob(p,2.2))
                put(cv,lay,x+wl/2,y+sz*0.62+(1-eob(p,2.0))*64-52*q,alpha=eo3(seg(p,0,0.45))*(1-q),sx=sc*(1+0.10*b),sy=sc*(1-0.10*b))
            x+=wl+sp
        y+=int(sz*1.12)+(18 if style!='sub' else 0)
    if snd and abs(t-t0)<0.5/FPS:pass

# ---------- phone
REST=(190,440);K0=700/1080
_SH={}
def draw_phone(cv,src,tl,K,zoomed=0.0,shadow_a=1.0):
    sw=int(1080*K);sh=int(2344*K);x,y=int(tl[0]),int(tl[1])
    r=int(sw*0.095);bt=max(4,int(sw*0.021))
    if zoomed<0.6 and shadow_a>0.02:
        key=(sw//10,round(shadow_a,1))
        if key not in _SH:
            base=shadow_rr(sw,sh,r+bt,blur=34,off=28,alpha=int(85*shadow_a),pad=90);_SH[key]=base
        s=_SH[key];cv.paste(solid(s.width,s.height,(70,48,22)),(x-90,y-90),s)
    cv.paste(solid(sw+2*bt,sh+2*bt,(20,20,22)),(x-bt,y-bt),rmask(sw+2*bt,sh+2*bt,r+bt))
    cv.paste(src.resize((sw,sh),Image.BILINEAR),(x,y),rmask(sw,sh,r))

class Clip:
    def __init__(s,path,ss,to,speed):
        s.proc=subprocess.Popen([FF,'-v','error','-ss',str(ss),'-t',str(to-ss),'-i',S+'/'+path,'-vf',f'setpts=(PTS-STARTPTS)/{speed},fps=30','-f','rawvideo','-pix_fmt','rgb24','-'],stdout=subprocess.PIPE)
        s.last=Image.new('RGB',(1080,2344),(10,10,20))
    def next(s):
        n=1080*2344*3;b=s.proc.stdout.read(n)
        if len(b)==n:s.last=Image.frombytes('RGB',(1080,2344),b)
        return s.last
    def close(s):
        try:s.proc.kill()
        except Exception:pass
def clip_at(path,ss,t_local,speed):
    r=subprocess.run([FF,'-v','error','-ss',str(ss+t_local*speed),'-i',S+'/'+path,'-frames:v','1','-f','image2pipe','-vcodec','png','-'],capture_output=True)
    return Image.open(io.BytesIO(r.stdout)).convert('RGB')

# ---------- timeline
class Shot:
    def __init__(s,t0,kind,f,ss,to,speed,dur=None):
        s.t0=t0;s.kind=kind;s.f=f;s.ss=ss;s.to=to;s.speed=speed;s.dur=dur if dur else (to-ss)/speed;s.t1=t0+s.dur
    def gt(s,src_t): return s.t0+(src_t-s.ss)/s.speed
HOOK_END=4.6
PH_IN=4.4
A1=Shot(HOOK_END,'clip','m1.mp4',10,22,2)
A2=Shot(A1.t1,'clip','m1.mp4',22,36,2.2)
A3=Shot(A2.t1,'clip','m1.mp4',51,84,2.4)
A4=Shot(A3.t1,'clip','m1.mp4',104,118,2)
A5=Shot(A4.t1,'clip','m2.mp4',49,56,1.4)
A6=Shot(A5.t1,'clip','m3.mp4',3.5,13,1.2)
A7=Shot(A6.t1,'still','a2.png',0,0,1,dur=3.6)
SHOTS=[A1,A2,A3,A4,A5,A6,A7]
PH_OUT=A7.t1;PH_OUT_END=PH_OUT+0.8
NODE0=PH_OUT+0.3;NODE1=NODE0+10.0;PRIV1=NODE1+7.8;END1=PRIV1+5.0
TOTAL=END1
# zoom windows (in start, in done, out start, out done, focal source y)
ZOOMS=[(A3.gt(51.0),A3.gt(51.0)+0.6,A3.gt(63.5),A3.gt(63.5)+0.6,1900),
       (A3.gt(73.0),A3.gt(73.0)+0.6,A3.gt(83.2),A3.gt(83.2)+0.6,1700),
       (A4.gt(111.0),A4.gt(111.0)+0.6,A4.gt(117.0),A4.gt(117.0)+0.5,300),
       (A7.t0,A7.t0+0.5,A7.t1-0.35,A7.t1,1160)]
def zoom_amt(t):
    for a,b,c,d,fy in ZOOMS:
        if a<=t<=d:return eio3(seg(t,a,b))*(1-eio3(seg(t,c,d))),fy
    return 0.0,1172
def phone_tf(t):
    z,fy=zoom_amt(t)
    pin=eob(seg(t,PH_IN,PH_IN+0.8),0.9);pout=ei3(seg(t,PH_OUT,PH_OUT_END))
    dy=(H-REST[1]+80)*((1-pin)+pout)
    amb=5*math.sin(t*1.7)*(1-z)*clamp(seg(t,PH_IN+1.0,PH_IN+1.6))*(1-pout)
    s=lerp(1.0,1.56,z);K=K0*s
    zx=(W-1080*K)/2;zy=clamp(1000-fy*K,H-2344*K,0)
    tl=(lerp(REST[0],zx,z),lerp(REST[1],zy,z)+dy+amb)
    return tl,K,z,(pin>0.001 and pout<0.999),(1-pin),pout

# ---------- hook
random.seed(7)
def make_strokes():
    strokes=[]
    for line in range(8):
        x=60
        while x<500:
            wl=random.randint(50,120);word=[]
            for i in range(wl//3):
                word.append((x+i*3,line*62+9*math.sin(i*0.9+random.random())+random.uniform(-3,3)))
            strokes.append(word);x+=wl+random.randint(16,26)
    return strokes
STROKES=make_strokes()
@lru_cache(None)
def pen_layer():
    pen=Image.new('RGBA',(60,380),(0,0,0,0));d=ImageDraw.Draw(pen)
    d.rounded_rectangle([16,10,44,310],radius=12,fill=(30,30,36,255));d.rounded_rectangle([16,70,44,96],radius=4,fill=ACC+(255,))
    d.polygon([(16,310),(44,310),(30,364)],fill=(224,198,152,255));d.polygon([(25,348),(35,348),(30,364)],fill=(30,30,36,255))
    return pen
def rot(layer,ang): return layer.rotate(ang,expand=True,resample=Image.BICUBIC)
@lru_cache(64)
def notebook_base(p100):
    w,h=640,820;pad=80;p=p100/100
    lay=Image.new('RGBA',(w+2*pad,h+2*pad),(0,0,0,0))
    sh=shadow_rr(w,h,26,blur=30,off=20,alpha=80,pad=pad);lay.paste((60,40,20,255),(0,0),sh)
    page=Image.new('RGB',(w,h),PAPER);d=ImageDraw.Draw(page)
    for yy in range(110,h-40,62):d.line([(0,yy),(w,yy)],fill=(206,214,228),width=2)
    d.line([(70,0),(70,h)],fill=(232,150,150),width=3)
    for yy in range(70,h-30,150):d.ellipse([22,yy,48,yy+26],fill=(214,206,190))
    total=sum(len(s) for s in STROKES);n=int(total*p);cnt=0;head=(60,100)
    for s in STROKES:
        if cnt>=n:break
        take=min(len(s),n-cnt);pts=[(x+40,y+100) for x,y in s[:take]]
        if len(pts)>1:d.line(pts,fill=(40,56,140),width=4,joint='curve')
        head=pts[-1] if pts else head;cnt+=take
    lay.paste(page,(pad,pad),rmask(w,h,26))
    return lay,(head[0]+pad,head[1]+pad)
def notebook(p,tilt):
    lay,head=notebook_base(int(p*100))
    lay=lay.copy()
    if p<1:
        pen=rot(pen_layer(),-28);lay.paste(pen,(int(head[0]-10),int(head[1]-pen.height+22)),pen)
    return rot(lay,tilt)
@lru_cache(None)
def stamp_layer():
    w,h=730,210;lay=Image.new('RGBA',(w,h),(0,0,0,0));d=ImageDraw.Draw(lay)
    d.rounded_rectangle([6,6,w-6,h-6],radius=34,outline=RED+(255,),width=14)
    f=font(FD,112);tw=f.getlength('MISSED IT');d.text(((w-tw)/2,40),'MISSED IT',font=f,fill=RED+(255,))
    return lay
BUBBLES=[('Decision: ship Friday',BLUE,0.9,760),('Action: send the deck',AMBER,1.25,980),('Budget is approved',GREEN,1.6,1190)]
def scene_hook(cv,t):
    # act 1: anticipation (frantic, a little too fast)
    nb_in=eob(seg(t,0.0,0.5),2.2);fall=ei3(seg(t,3.2,3.9))
    if nb_in>0.01 and fall<1:
        p=seg(t,0.2,2.6)
        shake=0
        if 2.45<t<2.9:shake=14*math.sin((t-2.45)*70)*(1-seg(t,2.45,2.9))
        lay=notebook(clamp(p),-4+2*math.sin(t*1.1)+fall*14)
        put(cv,lay,540+shake,1250+(1-nb_in)*600+fall*1100,alpha=1.0,sx=lerp(0.7,1,nb_in))
    # words that fly past the pen
    for text,col,t0,y in BUBBLES:
        p=seg(t,t0,t0+1.15)
        if 0<p<1:
            x=lerp(1350,-330,eio3(p));arc=-70*math.sin(math.pi*p)
            put(cv,card_layer(text,40,FSB,34,20,dot=col),x,y+arc,1.0)
    headline(cv,t,0.1,1.55,[('h','I take notes'),('h','on paper.')])
    headline(cv,t,2.0,3.3,[('h','I write.'),('red','And miss it.')],stag=0.05)
    # stamp slam
    if t>2.4:
        p=seg(t,2.4,2.85);sc=2.6-1.6*eob(p,1.3);a=clamp(p*5)
        if t>3.3:a*=1-ei3(seg(t,3.3,3.7))
        lay=rot(stamp_layer(),-9)
        put(cv,lay,560,1290,a,sx=sc*(1+0.08*bump(seg(t,2.85,3.2))),sy=sc*(1-0.08*bump(seg(t,2.85,3.2))))
    # pen flick
    if 3.1<t<4.3:
        p=seg(t,3.1,4.2);x=lerp(700,1250,eo3(p));y=lerp(1000,300,p)-300*math.sin(math.pi*p)
        put(cv,rot(pen_layer(),-28+p*500),x,y,alpha=1-seg(t,3.9,4.3))
    # act 2 setup: the promise
    if t>3.4:
        a=eo3(seg(t,3.5,3.9))*(1-ei3(seg(t,PH_IN-0.3,PH_IN+0.1)));yo=-ei3(seg(t,PH_IN-0.3,PH_IN+0.1))*140
        put(cv,tlayer('So I built this.',FM,50,INK2),540,700+yo+(1-eob(seg(t,3.5,3.9),2))*40,alpha=a)
        pw=eob(seg(t,3.6,4.1),1.6)
        put(cv,tlayer('Marginalia',SERIF,236,INK),540,880+yo,alpha=a,sx=lerp(0.85,1,pw),sy=lerp(0.85,1,pw))

# ---------- on-phone text
def caption_pill(cv,t,t0,t1,text,pos='top'):
    y=190 if pos=='top' else 1720
    lay=card_layer(text,52)
    if t<t0 or t>t1+0.25:return
    p=seg(t,t0,t0+0.34);a=eo3(seg(t,t0,t0+0.15));s=lerp(0.85,1,eob(p,2.0));yo=(1-eob(p,2.0))*(-90 if pos=='top' else 90)
    q=ei3(seg(t,t1,t1+0.22));a*=1-q;yo+=(-50 if pos=='top' else 50)*q
    put(cv,lay,540,y+yo,a,s*(1+0.06*bump(p)),s*(1-0.06*bump(p)))
def chip(cv,t,t0,t1,text,cx,cy,dot=ACC,size=38):
    pop(cv,card_layer(text,size,FSB,34,20,dot=dot),cx,cy,t,t0,t1,ph=cx*0.01)
def scene_phone_text(cv,t):
    headline(cv,t,A1.t0+0.1,A1.t1-0.2,[('h','Press record.'),('acc','Pen down.')])
    headline(cv,t,A2.t0+0.1,A2.t1-0.25,[('h','Captions appear'),('h','as people talk.')])
    chip(cv,t,A2.t0+1.5,A2.t1-0.2,'No cloud',840,560,GREEN)
    j0=A3.gt(51)
    caption_pill(cv,t,j0+0.7,j0+2.2,'Tap Note, Action or Decision')
    caption_pill(cv,t,j0+2.35,A3.gt(63.2),'The AI tidies it up')
    caption_pill(cv,t,A3.gt(74.5),A3.gt(83.0),'Old meetings chime in')
    headline(cv,t,A4.t0+0.1,A4.gt(110.6),[('h','Hold to stop.')])
    caption_pill(cv,t,A4.gt(111.6),A4.gt(117.2),'Notes are ready','bottom')
    headline(cv,t,A5.t0+0.1,A5.t1-0.25,[('h','A summary, notes'),('h','and transcript.')])
    chip(cv,t,A5.t0+1.5,A5.t1-0.2,'Written on a compute node',640,1830,BLUE)
    headline(cv,t,A6.t0+0.1,A6.t1-0.25,[('h','Then just ask.'),('sub','What did we decide?')])
    caption_pill(cv,t,A7.t0+0.6,A7.t1-0.4,'Every answer shows its sources')

# ---------- node scene (surprise beat)
def fmt_time(sec):
    s=int(round(sec));return f'{s//60}:{s%60:02d}'
def laptop(cv,cx,cy,led,label):
    w,h=420,270
    sh=shadow_rr(w,h,24,blur=24,off=16,alpha=70,pad=60);cv.paste(solid(sh.width,sh.height,(70,48,22)),(cx-w//2-60,cy-h//2-60),sh)
    cv.paste(solid(w,h,(28,27,32)),(cx-w//2,cy-h//2),rmask(w,h,24))
    cv.paste(solid(w-26,h-26,(40,40,60)),(cx-w//2+13,cy-h//2+13),rmask(w-26,h-26,16))
    bw,bh=500,26;cv.paste(solid(bw,bh,(196,190,178)),(cx-bw//2,cy+h//2),rmask(bw,bh,13))
    d=ImageDraw.Draw(cv);r=14;d.ellipse([cx-w//2+40,cy-r-6,cx-w//2+40+2*r,cy+r-6],fill=led)
    l=tlayer(label,FSB,42,(240,240,250));cv.paste(l,(cx-w//2+82,cy-34),l)
def burst(cv,cx,cy,u,col):
    if u<=0 or u>=1:return
    d=ImageDraw.Draw(cv)
    for i in range(16):
        ang=i*2*math.pi/16+0.3;r=eo5(u)*(140+30*(i%3));rad=max(1,int(11*(1-u)))
        x=cx+math.cos(ang)*r;y=cy+math.sin(ang)*r+u*u*40
        d.ellipse([x-rad,y-rad,x+rad,y+rad],fill=col if i%2 else AMBER)
def scene_node(cv,t0,t,still):
    u=t-t0
    headline(cv,t,t0+0.1,t0+4.3,[('h','Working out'),('h','who said what was slow.')],y=110)
    headline(cv,t,t0+4.7,t0+9.6,[('h','So I tuned it'),('acc','on my own PC.')],y=110)
    # phone mini and laptop enter
    pw=250;ph=int(pw*2344/1080);px,py=230,1200
    ent=eob(seg(u,0.1,0.7),1.7)
    m=still.resize((pw,ph),Image.BILINEAR);bt=7;ox=px-pw//2;oy=py-ph//2+int((1-ent)*500)
    sh=shadow_rr(pw,ph,28,blur=22,off=14,alpha=80,pad=50);cv.paste(solid(sh.width,sh.height,(70,48,22)),(ox-50,oy-50),sh)
    cv.paste(solid(pw+2*bt,ph+2*bt,(20,20,22)),(ox-bt,oy-bt),rmask(pw+2*bt,ph+2*bt,28+bt));cv.paste(m,(ox,oy),rmask(pw,ph,28))
    put(cv,tlayer('your phone',FM,36,INK2),px,py+ph//2+56,alpha=ent)
    lx,ly=790,1170;lent=eob(seg(u,0.3,0.9),1.7);ly2=ly+int((1-lent)*500)
    led=GREEN;label='Ready'
    if 3.5<=u<5.3:led=BLUE;label='Working'
    if 7.2<=u<8.4:led=AMBER;label='Paused'
    if lent>0.02:
        laptop(cv,lx,ly2,led,label);put(cv,tlayer('your PC',FM,36,INK2),lx,ly2+250,alpha=lent)
    # stopwatch card (the surprise): counts up slowly on the phone, snaps down on the PC
    if u>0.9:
        if u<4.6:
            sec=545*eo3(seg(u,1.0,3.3)) ;col=RED;lab='19 minute talk, my first try'
            if 3.3<u<3.45:pass
            card=card_layer(fmt_time(sec),170,FD,70,34,color=col)
            lbl=lab
        else:
            sec=lerp(545,22.7,eo5(seg(u,4.6,5.2)));col=GREEN;lbl='Same talk, tuned, on my PC'
            card=card_layer(fmt_time(sec),170,FD,70,34,color=GREEN)
        p=eob(seg(u,0.9,1.3),2.0);land=seg(u,4.6,5.5)
        sq=1+0.12*bump(seg(u,5.15,5.6)) if u>5.1 else 1
        put(cv,card,540,620,alpha=eo3(seg(u,0.9,1.2)),sx=lerp(0.7,1,p)*sq,sy=lerp(0.7,1,p)/sq)
        put(cv,tlayer(lbl,FM,40,INK2),540,790,alpha=eo3(seg(u,1.1,1.5)))
        burst(cv,540,620,seg(u,5.15,6.0),GREEN)
    # packets arc from phone to PC
    def along(p):return lerp(px+140,lx-250,p)
    for i,(txt,col,ts) in enumerate([('audio',BLUE,3.2),('photo',AMBER,3.45),('question',ACC,3.7)]):
        p=seg(u,ts,ts+0.9)
        if 0<p<1:put(cv,card_layer(txt,32,FSB,26,14,dot=col),along(eio3(p)),1120-150*math.sin(math.pi*p)+i*40,alpha=1-ei3(seg(p,0.85,1.0)))
    p=seg(u,5.3,6.1)
    if 0<p<1:put(cv,card_layer('answer',32,FSB,26,14,dot=GREEN),lerp(lx-250,px+140,eio3(p)),1120-120*math.sin(math.pi*p))
    d=ImageDraw.Draw(cv)
    if u>1.2:
        for i in range(14):
            x=px+150+i*(lx-250-px-150)/13;d.ellipse([x-4,1115-4,x+4,1115+4],fill=(190,182,166))
    if u>6.0:
        pop(cv,card_layer('The PC keeps nothing',46,FD,44,26,dot=GREEN),540,1690,t,t0+6.0,None,float_amp=5)
    if u>7.0:
        put(cv,tlayer('Pause it or stop it any time.',FM,40,INK2),540,1800,alpha=eo3(seg(u,7.0,7.5)))

# ---------- private (trust beat)
def padlock(cv,cx,cy,k,drop,squash):
    bw,bh=170,140
    sh=Image.new('RGBA',(200,200),(0,0,0,0));sd=ImageDraw.Draw(sh);lift=int((1-drop)*40)
    sd.arc([30,10-lift,170,170-lift],180,360,fill=INK+(255,),width=22)
    sd.rectangle([30,90-lift,52,130-lift],fill=INK+(255,));sd.rectangle([148,90-lift,170,130-lift],fill=INK+(255,))
    put(cv,sh,cx,cy-80+squash*10,sx=k)
    body=Image.new('RGBA',(bw,bh),(0,0,0,0));body.paste(solid(bw,bh,ACC),(0,0),rmask(bw,bh,30))
    bd=ImageDraw.Draw(body);bd.ellipse([bw/2-14,bh/2-22,bw/2+14,bh/2+6],fill=CARD);bd.rectangle([bw/2-6,bh/2-4,bw/2+6,bh/2+34],fill=CARD)
    put(cv,body,cx,cy+22,sx=k*(1+0.10*squash),sy=k*(1-0.10*squash))
def check_row(cv,t,t0,text,y):
    p=seg(t,t0,t0+0.4)
    if p<=0:return
    lay=tlayer(text,FD,58,INK);s=eob(p,2.2)
    c=Image.new('RGBA',(84,84),(0,0,0,0));cd=ImageDraw.Draw(c);cd.ellipse([2,2,82,82],fill=ACC+(255,))
    dp=seg(t,t0+0.15,t0+0.5)  # checkmark stroke draws in
    pts=[(24,44),(37,58),(61,28)]
    if dp>0:
        if dp<0.5:end=(lerp(pts[0][0],pts[1][0],dp*2),lerp(pts[0][1],pts[1][1],dp*2));cd.line([pts[0],end],fill=(255,255,255,255),width=9)
        else:
            e2=(lerp(pts[1][0],pts[2][0],(dp-.5)*2),lerp(pts[1][1],pts[2][1],(dp-.5)*2));cd.line([pts[0],pts[1],e2],fill=(255,255,255,255),width=9,joint='curve')
    xo=(1-eo3(p))*-70
    put(cv,c,162+xo,y,alpha=eo3(seg(p,0,.4)),sx=s*(1+0.10*bump(p)),sy=s*(1-0.10*bump(p)))
    put(cv,lay,230+lay.width/2+(1-eo3(p))*70,y-4,alpha=eo3(seg(p,0,.5)))
def scene_private(cv,t0,t):
    u=t-t0
    k=eob(seg(u,0.1,0.55),2.0);drop=eo3(seg(u,0.5,0.62));sq=bump(seg(u,0.6,1.0))
    padlock(cv,540,330,k,drop,sq)
    headline(cv,t,t0+0.7,t0+30,[('hs','Private by design.')],y=520,stag=0.07)
    rows=['The AI runs on your phone','No account and no cloud','The PC keeps nothing','Pause or stop it any time']
    for i,r in enumerate(rows):check_row(cv,t,t0+1.5+i*0.45,r,930+i*200)

# ---------- end (resolution: calm, premium)
def pen_sim(u):
    g=3600.0;y=-380.0;v=0.0;ground=1500.0;dt=1/240;tt=0.0
    while tt<u:
        v+=g*dt;y+=v*dt;tt+=dt
        if y>=ground:y=ground;v=-v*0.34
        if abs(v)<30 and y>=ground-1:v=0
    return y
def scene_end(cv,t0,t):
    u=t-t0
    a=eo3(seg(u,0.1,0.9));sc=lerp(0.96,1.0,eo3(seg(u,0.1,1.2)))
    put(cv,tlayer('Marginalia',SERIF,236,INK),540,720+(1-eo3(seg(u,0.1,1.0)))*40,alpha=a,sx=sc)
    put(cv,tlayer('Put the pen down.',SERIFI,100,ACC),540,930,alpha=eo3(seg(u,0.9,1.7)))
    put(cv,tlayer('Meeting notes for Android. Private by design.',FM,40,INK2),540,1760,alpha=eo3(seg(u,2.4,3.2)))
    if u>0.5:
        y=pen_sim(u-0.5)
        ang=lerp(35,100,eo3(seg(u,0.5,1.6)))
        sh=Image.new('RGBA',(420,60),(0,0,0,0));ImageDraw.Draw(sh).ellipse([30,15,390,45],fill=(40,30,20,70));sh=sh.filter(ImageFilter.GaussianBlur(8))
        put(cv,sh,560,1488,alpha=clamp((y+380)/1800),sx=lerp(0.4,1,clamp((y+380)/1880)))
        put(cv,rot(pen_layer(),ang),560,y-60)

# ---------- compose
STILLS={}
def getstill(n):
    if n not in STILLS:STILLS[n]=Image.open(S+'/'+n).convert('RGB')
    return STILLS[n]
def render(t,src):
    tl,K,z,vis,pushin,pout=phone_tf(t)
    cv=bg_canvas(t,push=-(pushin*1.0)+pout).copy() if False else bg_canvas(t,push=(1-pushin)*0+(-pushin)+pout)
    if t<PH_IN+1.0:scene_hook(cv,t)
    if vis and src is not None:
        sa=eo3(seg(t,PH_IN+0.7,PH_IN+1.2))
        draw_phone(cv,src,tl,K,z,shadow_a=sa)
    scene_phone_text(cv,t)
    if NODE0<=t<NODE1:scene_node(cv,NODE0,t,getstill('a5.png'))
    if NODE1-0.01<=t<PRIV1:scene_private(cv,NODE1,t)
    if t>=PRIV1-0.01:scene_end(cv,PRIV1,t)
    return cv
def shot_at(t):
    for s in SHOTS:
        if s.t0<=t<s.t1:return s
    return None
def test(times):
    for t in times:
        s=shot_at(t);src=None
        if s:src=getstill(s.f) if s.kind=='still' else clip_at(s.f,s.ss,t-s.t0,s.speed)
        elif t>=SHOTS[-1].t1:src=getstill('a2.png')
        render(t,src).save(f'{OUT}/frames/t_{int(t*10):04d}.png')

# ---------- sound (soft, matches motion)
SR=44100
def env(n,a=0.004,d=None):
    d=d or n/SR/3;t=np.arange(n)/SR;return np.minimum(1,t/a)*np.exp(-t/d)
def snd(kind):
    if kind=='pop':
        n=int(0.12*SR);t=np.arange(n)/SR;f=520+700*np.exp(-t*40);return np.sin(2*np.pi*np.cumsum(f)/SR)*env(n,0.002,0.03)*0.45
    if kind=='tick':
        n=int(0.03*SR);t=np.arange(n)/SR;return np.sin(2*np.pi*1800*t)*env(n,0.001,0.006)*0.25
    if kind=='whoosh':
        n=int(0.45*SR);x=np.random.RandomState(1).randn(n);k=np.ones(60)/60;x=np.convolve(x,k,'same');x=np.convolve(x,np.ones(30)/30,'same')*40
        e=np.sin(np.pi*np.arange(n)/n)**2;return x*e*0.5
    if kind=='thud':
        n=int(0.35*SR);t=np.arange(n)/SR;return (np.sin(2*np.pi*(70+90*np.exp(-t*25))*t)*env(n,0.002,0.07)*0.9)+np.random.RandomState(2).randn(n)*env(n,0.001,0.01)*0.15
    if kind=='chime':
        n=int(0.9*SR);t=np.arange(n)/SR;return (np.sin(2*np.pi*880*t)+0.6*np.sin(2*np.pi*1318*t))*env(n,0.004,0.25)*0.28
    if kind=='click':
        n=int(0.05*SR);t=np.arange(n)/SR;return (np.sin(2*np.pi*2400*t)*env(n,0.001,0.008)+np.random.RandomState(3).randn(n)*env(n,0.0005,0.003)*0.4)*0.5
    if kind=='bounce':
        n=int(0.2*SR);t=np.arange(n)/SR;return np.sin(2*np.pi*(180+80*np.exp(-t*30))*t)*env(n,0.001,0.04)*0.6
    return np.zeros(100)
def build_audio(path):
    n=int(TOTAL*SR)+SR;buf=np.zeros(n)
    for t,k,g in SFX:
        s=snd(k)*g;i=int(t*SR);j=min(n,i+len(s))
        if i<n:buf[i:j]+=s[:j-i]
    buf=buf/max(1e-9,float(np.abs(buf).max()))*0.55;pcm=(buf*32767).astype('<i2')
    with wave.open(path,'wb') as w:w.setnchannels(1);w.setsampwidth(2);w.setframerate(SR);w.writeframes(pcm.tobytes())
def plan_sfx():
    # hook
    sfx(0.1,'pop',.6);sfx(1.0,'whoosh',.5);sfx(1.3,'whoosh',.5);sfx(1.6,'whoosh',.5);sfx(2.0,'pop',.6)
    sfx(2.55,'thud',1.0);sfx(3.1,'whoosh',.6);sfx(3.5,'pop',.7);sfx(PH_IN,'whoosh',.8);sfx(PH_IN+0.8,'bounce',.5)
    for h in [A1.t0+.1,A2.t0+.1,A4.t0+.1,A5.t0+.1,A6.t0+.1]:sfx(h,'pop',.5)
    for z in ZOOMS:sfx(z[0],'whoosh',.45)
    for c in [A2.t0+1.5,A5.t0+1.5]:sfx(c,'pop',.5)
    j0=A3.gt(51)
    for c in [j0+.7,j0+2.35,A3.gt(74.5),A4.gt(111.6),A7.t0+.6]:sfx(c,'pop',.5)
    sfx(PH_OUT,'whoosh',.6)
    n=NODE0
    sfx(n+.1,'pop',.5);sfx(n+4.1,'pop',.5);sfx(n+1.0,'pop',.4)
    k=n+1.0
    while k<n+3.3:sfx(k,'tick',.5);k+=0.07
    for i,ts in enumerate([3.2,3.45,3.7]):sfx(n+ts,'whoosh',.25)
    k=n+4.6
    while k<n+5.2:sfx(k,'tick',.5);k+=0.04
    sfx(n+5.15,'chime',1.0);sfx(n+6.0,'pop',.6)
    p=NODE1
    sfx(p+.1,'pop',.5);sfx(p+.6,'click',1.0);sfx(p+.7,'pop',.5)
    for i in range(4):sfx(p+1.5+i*.45,'pop',.5)
    e=PRIV1+0.5;u=0.0;g=3600.0;y=-380.0;v=0.0;dt=1/240
    tt=0.0;lastb=-1
    while tt<1.6:
        v+=g*dt;y+=v*dt;tt+=dt
        if y>=1500:
            y=1500;
            if abs(v)>120 and tt-lastb>0.1:sfx(e+tt,'bounce',min(1,abs(v)/2000));lastb=tt
            v=-v*0.34
    return
def full(sound):
    out=OUT+'/marginalia-demo.mp4'
    tmp=S+'/silent.mp4'
    enc=subprocess.Popen([FF,'-y','-v','error','-f','rawvideo','-pix_fmt','rgb24','-s',f'{W}x{H}','-r','30','-i','-','-c:v','libx264','-pix_fmt','yuv420p','-crf','17','-preset','medium','-movflags','+faststart',tmp],stdin=subprocess.PIPE)
    n=int(TOTAL*FPS);cur=None;clip=None
    for i in range(n):
        t=i/FPS;s=shot_at(t);src=None
        if s is not cur:
            if clip:clip.close();clip=None
            cur=s
            if s and s.kind=='clip':clip=Clip(s.f,s.ss,s.to,s.speed)
        if s:src=clip.next() if clip else getstill(s.f)
        elif t>=SHOTS[-1].t1:src=getstill('a2.png')
        enc.stdin.write(render(t,src).tobytes())
        if i%150==0:print(i,n,flush=True)
    enc.stdin.close();enc.wait()
    os.replace(tmp,OUT+'/marginalia-demo.mp4')
    print('done',TOTAL)

# ===== calm overrides: premium feel, no squash, no jitter =====
def ease(x): return eo5(x)
def bg_canvas(t,push=0.0):
    return BG.crop((210,190,210+W,190+H))
def pop(cv,layer,cx,cy,t,t0,t1=None,float_amp=0,ph=0.0):
    if t<t0 or (t1 is not None and t>t1+0.3):return
    p=ease(seg(t,t0,t0+0.5));a=eo3(seg(t,t0,t0+0.35));s=lerp(0.94,1.0,p);yo=(1-p)*24
    if t1 is not None:
        q=eo3(seg(t,t1,t1+0.3));a*=1-q;yo-=12*q
    put(cv,layer,cx,cy+yo,a,s,s)
def caption_pill(cv,t,t0,t1,text,pos='top'):
    y=190 if pos=='top' else 1720
    pop(cv,card_layer(text,52),540,y,t,t0,t1)
def headline(cv,t,t0,t1,lines,y=110,stag=0.08,snd=True):
    if t<t0 or t>t1+0.4:return
    q=eo3(seg(t,t1,t1+0.3))
    for i,(style,text) in enumerate(lines):
        fp,sz,col=STY[style];lay=tlayer(text,fp,sz,col)
        p=ease(seg(t,t0+i*stag,t0+i*stag+0.6));a=eo3(seg(t,t0+i*stag,t0+i*stag+0.4))*(1-q)
        cy=y+sz*0.62+(1-p)*40-16*q
        if style in('acc','red') and a>0.05:
            f=font(fp,sz);wl=f.getlength(text);hw=int(wl*ease(seg(t,t0+i*stag+0.35,t0+i*stag+0.95)))
            if hw>4:
                hl=Image.new('RGBA',(hw,int(sz*0.42)),(HL if style=='acc' else (255,214,210))+(255,));m=rmask(hw,int(sz*0.42),12)
                hl.putalpha(m.point(lambda v:int(v*a)));cv.paste(hl,(int((W-wl)/2),int(y+sz*0.78+(1-p)*40-16*q)),hl)
        put(cv,lay,W/2,cy,a)
        y+=int(sz*1.12)+(18 if style!='sub' else 0)
def phone_tf(t):
    z,fy=zoom_amt(t)
    pin=ease(seg(t,PH_IN,PH_IN+0.9));pout=ei3(seg(t,PH_OUT,PH_OUT_END))
    dy=(H-REST[1]+80)*((1-pin)+pout)
    s=lerp(1.0,1.56,z);K=K0*s
    zx=(W-1080*K)/2;zy=clamp(1000-fy*K,H-2344*K,0)
    tl=(lerp(REST[0],zx,z),lerp(REST[1],zy,z)+dy)
    return tl,K,z,(pin>0.001 and pout<0.999),(1-pin),pout
def scene_hook(cv,t):
    nb_in=ease(seg(t,0.0,0.7));out=eo3(seg(t,3.1,3.7))
    if nb_in>0.01 and out<1:
        p=seg(t,0.3,2.6)
        lay=notebook(clamp(p),-3)
        put(cv,lay,540,1250+(1-nb_in)*120+out*80,alpha=nb_in*(1-out))
    for text,col,t0,y in BUBBLES:
        p=seg(t,t0,t0+1.6)
        if 0<p<1:
            x=lerp(1350,-330,eio3(p))
            put(cv,card_layer(text,40,FSB,34,20,dot=col),x,y,1.0)
    headline(cv,t,0.15,1.5,[('h','I take notes'),('h','on paper.')])
    headline(cv,t,1.85,3.1,[('h','I write.'),('red','And miss it.')])
    if t>2.3 and t<3.8:
        p=ease(seg(t,2.3,2.8));a=eo3(seg(t,2.3,2.6))*(1-eo3(seg(t,3.1,3.6)))
        put(cv,rot(stamp_layer(),-8),560,1290,a,lerp(1.12,1,p))
    if t>3.4:
        a=eo3(seg(t,3.5,4.0))*(1-eo3(seg(t,PH_IN-0.3,PH_IN+0.1)));p=ease(seg(t,3.5,4.1))
        put(cv,tlayer('So I built this.',FM,50,INK2),540,700+(1-p)*30,alpha=a)
        put(cv,tlayer('Marginalia',SERIF,236,INK),540,880+(1-p)*30,alpha=a)
def burst(cv,cx,cy,u,col): return

if __name__=='__main__':
    if sys.argv[1]=='test':test([float(x) for x in sys.argv[2:]])
    else:full(True)
