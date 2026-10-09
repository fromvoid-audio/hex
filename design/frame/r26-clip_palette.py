"""A bounded four-hue OKLCH palette for the graphite workstation theme."""
import math

from theme_palette import THEME, FAMILY_SPECS, TONES, UTILITIES
FAMILIES = [(name,hue) for name,hue,_ in FAMILY_SPECS]


def rgb_from_oklch(lightness, chroma, hue):
    """Map out-of-gamut shades by reducing chroma, preserving lightness/hue."""
    angle = math.radians(hue)
    for _ in range(80):
        a, b = chroma * math.cos(angle), chroma * math.sin(angle)
        l = (lightness + 0.3963377774 * a + 0.2158037573 * b) ** 3
        m = (lightness - 0.1055613458 * a - 0.0638541728 * b) ** 3
        s = (lightness - 0.0894841775 * a - 1.2914855480 * b) ** 3
        linear = [4.0767416621*l - 3.3077115913*m + 0.2309699292*s,
                  -1.2684380046*l + 2.6097574011*m - 0.3413193965*s,
                  -0.0041960863*l - 0.7034186147*m + 1.7076147010*s]
        if all(0 <= v <= 1 for v in linear):
            def encode(v):
                return round(255 * (12.92*v if v <= 0.0031308 else 1.055*v**(1/2.4)-0.055))
            return '#' + ''.join(f'{encode(v):02x}' for v in linear)
        chroma *= 0.95
    raise ValueError('palette color cannot be mapped to sRGB')


def contrast(a, b):
    def luminance(color):
        values=[int(color[i:i+2],16)/255 for i in (1,3,5)]
        linear=[v/12.92 if v <= .04045 else ((v+.055)/1.055)**2.4 for v in values]
        return sum(v*w for v,w in zip(linear,[.2126,.7152,.0722]))
    x,y=sorted([luminance(a),luminance(b)])
    return (y+.05)/(x+.05)


def palette():
    entries=[]
    for tone, (lightness,chroma) in enumerate(TONES,1):
        for family,hue,strength in FAMILY_SPECS:
            value=rgb_from_oklch(lightness,chroma*strength,hue)
            entries.append(dict(id=f'{family}-{tone}',name=f'{family} {tone}',
                                family=family,tone=tone,role='identity',hex=value))
    utilities=UTILITIES
    entries.extend(dict(id=name,name=name,family='utility',tone=None,role='utility',hex=value)
                   for name,value in utilities)
    for item in entries:
        choices=['#20282c','#e8ede8']
        if max(contrast(item['hex'],v) for v in choices)<4.5:choices=['#11191d','#f4f7f2']
        if max(contrast(item['hex'],v) for v in choices)<4.5:choices=['#000000','#ffffff']
        item['labelColor']=max(choices,key=lambda value:contrast(item['hex'],value))
        item['labelContrast']=round(contrast(item['hex'],item['labelColor']),2)
    if len(entries)!=64 or len({c['hex'] for c in entries})!=64:
        raise ValueError('palette must contain exactly 64 distinct colors')
    if any(c['labelContrast']<4.5 for c in entries):
        raise ValueError('palette label contrast falls below 4.5:1')
    return dict(version=1,id='hex-clip-colors',theme='hex ember and sea glass',
                count=64,identityCount=56,utilityCount=8,
                construction='four OKLCH hue anchors with two chroma strengths and seven tonal levels each; sRGB gamut mapping by chroma reduction',
                usage='identity colors for clips/tracks/elements; utility colors for neutral or standout elements; selection/focus must use a separate outline, not color alone',
                colors=entries)
