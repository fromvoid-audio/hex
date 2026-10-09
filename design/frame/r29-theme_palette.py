"""Kanagawa Dragon adaptation for Frame design only, never application runtime.
Source: https://github.com/rebelot/kanagawa.nvim/blob/master/lua/kanagawa/colors.lua
Clip seeds and accents are upstream Dragon; softened UI roles and tone variants are Hex's.
"""
THEME = {
    'background': '#242221',
    'surface': '#292723',
    'well': '#1d1c19',
    'overlay': '#302e2b',
    'separator': '#181715',
    'field_border': '#1e1c1a',
    'text': '#b2b5af',
    'muted': '#9b9990',
    'orange': '#b6927b',
    'sea_glass': '#8ea4a2',
    'enabled': '#b98d7b',
    'waveform': '#282727',
    'piano_row': '#262421',
    'key_dark': '#625e5a',
    'key_light': '#a6a69c',
}
# Restrained warm-earth / green / aqua subset, preserving the narrower hue direction.
# Seeds provide hue only; all clip tones are darker, lower-chroma Hex variants.
FAMILY_SEEDS = [('clay', '#b6927b'), ('copper', '#b98d7b'),
                ('amber', '#c4b28a'), ('sand', '#c8c093'),
                ('sage', '#87a987'), ('moss', '#8a9a7b'),
                ('teal', '#8ea4a2'), ('sea', '#7a8382')]
TONES = [(-.28, .50), (-.22, .58), (-.16, .65), (-.10, .68),
         (-.04, .62), (.02, .50), (.08, .35)]
UTILITIES = [('ink', '#0d0c0c'), ('charcoal', THEME['background']),
             ('graphite', '#393836'), ('gray', '#625e5a'), ('silver', '#a6a69c'),
             ('chalk', '#c5c9c5'), ('white', '#dcd7ba'), ('signal', '#ff9e3b')]
