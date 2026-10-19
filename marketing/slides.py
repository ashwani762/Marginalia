from PIL import Image, ImageDraw, ImageFont, ImageFilter
import os
HERE=os.path.dirname(os.path.abspath(__file__))
REPO=os.path.dirname(HERE)
S=os.environ.get('MARGINALIA_RECORDINGS',os.path.join(HERE,'recordings'))  # m1.mp4, m2.mp4, m3.mp4, a2.png, a5.png, n1.png, n3.png, rec3.png
OUT=os.environ.get('MARGINALIA_OUT',os.path.join(HERE,'out'))
IMG=os.path.join(REPO,'docs/images')
APP=os.path.join(REPO,'app/src/main/res/font')+'/'
os.makedirs(OUT,exist_ok=True)
FD='/usr/share/fonts/opentype/inter/InterDisplay-Bold.otf';FM='/usr/share/fonts/opentype/inter/Inter-Medium.otf'
def F(p,s):return ImageFont.truetype(p,s)
INK=(28,27,24);INK2=(112,106,94);ACC=(98,80,230);CARD=(255,253,248)
W,H=1080,1350
def bg():
    im=Image.new('RGB',(W,H));d=ImageDraw.Draw(im)
    for y in range(H):
        t=y/H;d.line([(0,y),(W,y)],fill=tuple(int(a*(1-t)+b*t) for a,b in zip((248,242,230),(240,232,216))))
    for box,col,a in [([-100,-80,600,500],(252,208,176),170),([500,850,1250,1500],(214,204,252),170)]:
        m=Image.new('L',(W,H),0);ImageDraw.Draw(m).ellipse(box,fill=a);m=m.filter(ImageFilter.GaussianBlur(130))
        im.paste(Image.new('RGB',(W,H),col),(0,0),m)
    return im
def rm(w,h,r):
    m=Image.new('L',(w*4,h*4),0);ImageDraw.Draw(m).rounded_rectangle([0,0,w*4-1,h*4-1],radius=r*4,fill=255);return m.resize((w,h),Image.LANCZOS)
def wrap(d,t,f,mw):
    L=[];c=''
    for w in t.split():
        x=(c+' '+w).strip()
        if d.textlength(x,font=f)<=mw:c=x
        else:L.append(c);c=w
    L.append(c);return L
def text(d,t,f,y,fill,mw=W-140,gap=1.2):
    for l in wrap(d,t,f,mw):
        d.text(((W-d.textlength(l,font=f))/2,y),l,font=f,fill=fill);y+=int(f.size*gap)
    return y
def phone(im,path,x,y,w,h):
    sh=Image.new('L',(w+160,h+160),0);ImageDraw.Draw(sh).rounded_rectangle([80,100,80+w,100+h],radius=40,fill=90);sh=sh.filter(ImageFilter.GaussianBlur(28))
    im.paste(Image.new('RGB',sh.size,(70,48,22)),(x-80,y-80),sh)
    bt=8;im.paste(Image.new('RGB',(w+2*bt,h+2*bt),(20,20,22)),(x-bt,y-bt),rm(w+2*bt,h+2*bt,44+bt))
    p=Image.open(path).convert('RGB').resize((w,h),Image.LANCZOS);im.paste(p,(x,y),rm(w,h,44))
def page(name,title,sub,imgs):
    im=bg();d=ImageDraw.Draw(im);y=text(d,title,F(FD,66),48,INK,gap=1.1);text(d,sub,F(FM,34),y+4,INK2)
    pw,ph=470,1020;top=275
    xs=[(W-pw)//2] if len(imgs)==1 else [(W-2*pw-40)//2,(W-2*pw-40)//2+pw+40]
    for x,p in zip(xs,imgs):phone(im,p,x,top,pw,ph)
    im.save(f'{OUT}/{name}.png')
# cover
im=bg();d=ImageDraw.Draw(im)
wm=F(APP+'instrument_serif.ttf',210);d.text(((W-d.textlength('Marginalia',font=wm))/2,110),'Marginalia',font=wm,fill=INK)
it=F(APP+'instrument_serif_italic.ttf',74);d.text(((W-d.textlength('Put the pen down.',font=it))/2,370),'Put the pen down.',font=it,fill=ACC)
text(d,'Meeting notes that stay on your phone',F(FM,40),490,INK2)
pw,ph=500,1085;phone(im,S+'/rec3.png',(W-pw)//2,620,pw,ph);im.save(OUT+'/1-cover.png')
page('2-live-and-notes','Live captions and notes','The AI listens on your phone and writes it up',[S+'/rec3.png',S+'/n1.png'])
page('3-ask-and-nodes','Ask your meetings','Answers come with their sources',[S+'/a5.png',IMG+'/nodes.png'])
im=bg();d=ImageDraw.Draw(im);t=F(APP+'instrument_serif.ttf',110);d.text(((W-d.textlength('Private by design.',font=t))/2,70),'Private by design.',font=t,fill=INK)
items=[('On your phone','The AI runs on the phone. Captions, notes and answers never need the internet'),('No account','Nothing to sign up for, nothing to leak'),('Nodes keep nothing','A compute node keeps no audio, no photos and no notes'),('You decide','Pause or stop a node any time')]
yy=260
for h,s in items:
    sh=Image.new('L',(W,260+120),0);ImageDraw.Draw(sh).rounded_rectangle([70,60+10,W-70,60+240],radius=32,fill=60);sh=sh.filter(ImageFilter.GaussianBlur(16))
    im.paste(Image.new('RGB',sh.size,(70,48,22)),(0,yy-60),sh)
    im.paste(Image.new('RGB',(W-140,235),CARD),(70,yy),rm(W-140,235,32));d=ImageDraw.Draw(im)
    d.text((115,yy+28),h,font=F(FD,48),fill=ACC);t2=yy+95
    for l in wrap(d,s,F(FM,38),W-230):d.text((115,t2),l,font=F(FM,38),fill=INK);t2+=50
    yy+=262
im.save(OUT+'/4-private-by-design.png')
