"""A bounded four-hue OKLCH palette for the graphite workstation theme."""
import math

from theme_palette import FAMILY_SEEDS, TONES, UTILITIES, SATURATION, TONE_NAMES
FAMILIES = [(name, seed) for name, seed in FAMILY_SEEDS]


def oklch_from_rgb(color):
    values = [int(color[i:i+2], 16)/255 for i in (1, 3, 5)]
    r, g, b = [v/12.92 if v <= .04045 else ((v+.055)/1.055)**2.4 for v in values]
    l = (.4122214708*r + .5363325363*g + .0514459929*b)**(1/3)
    m = (.2119034982*r + .6806995451*g + .1073969566*b)**(1/3)
    s = (.0883024619*r + .2817188376*g + .6299787005*b)**(1/3)
    lightness = .2104542553*l + .7936177850*m - .0040720468*s
    a = 1.9779984951*l - 2.4285922050*m + .4505937099*s
    b = .0259040371*l + .7827717662*m - .8086757660*s
    return lightness, math.hypot(a, b), math.degrees(math.atan2(b, a)) % 360


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
    for tone, offset,strength in TONES:
        for family,seed in FAMILY_SEEDS:
            lightness,chroma,hue=oklch_from_rgb(seed)
            value=rgb_from_oklch(min(.92,max(.30,lightness+offset)),chroma*strength*SATURATION[family],hue)
            entries.append(dict(id=f'{family}-{tone}',name=f'{family} {TONE_NAMES[tone]}',
                                family=family,tone=tone,variant=TONE_NAMES[tone],role='identity',hex=value))
    utilities=UTILITIES
    entries.extend(dict(id=name,name=name,family='utility',tone=None,role='utility',hex=value)
                   for name,value in utilities)
    for item in entries:
        choices=['#20282c','#e8ede8']
        if max(contrast(item['hex'],v) for v in choices)<4.5:choices=['#11191d','#f4f7f2']
        if max(contrast(item['hex'],v) for v in choices)<4.5:choices=['#000000','#ffffff']
        item['labelColor']=max(choices,key=lambda value:contrast(item['hex'],value))
        item['labelContrast']=round(contrast(item['hex'],item['labelColor']),2)
    if len(entries)!=36 or len({c['hex'] for c in entries})!=36:
        raise ValueError('palette must contain exactly 36 distinct colors')
    if any(c['labelContrast']<4.5 for c in entries):
        raise ValueError('palette label contrast falls below 4.5:1')
    return dict(version=1,id='hex-clip-colors',theme='hex orbital',
                count=36,identityCount=28,utilityCount=8,
                construction='four purple, blue, green and red seeds, each with seven paired lightness and chroma choices: dark soft/vivid, mid soft/core, bright vivid/soft and pale; sRGB gamut mapping by chroma reduction',
                usage='identity colors for clips/tracks/elements; utility colors for neutral or standout elements; selection/focus must use a separate outline, not color alone',
                colors=entries)
