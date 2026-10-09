"""Kanagawa Dragon adaptation for Frame design only, never application runtime.
Source: https://github.com/rebelot/kanagawa.nvim/blob/master/lua/kanagawa/colors.lua
Clip seeds and accents are upstream Dragon; softened UI roles and tone variants are Hex's.
"""
THEME = {
    'background': '#2c2a28',
    'surface': '#302e2b',
    'well': '#252320',
    'overlay': '#32302d',
    'separator': '#181715',
    'field_border': '#1e1c1a',
    'text': '#b2b5af',
    'muted': '#9b9990',
    'orange': '#b6927b',
    'sea_glass': '#8ea4a2',
    'enabled': '#b98d7b',
    'waveform': '#282727',
    'piano_row': '#2b2926',
    'key_dark': '#625e5a',
    'key_light': '#a6a69c',
}
# Restrained warm-earth / green / aqua subset, preserving the narrower hue direction.
# Seeds provide hue only; clip lightness remains dark while chroma is restored for clearer hue identity.
FAMILY_SEEDS = [('clay', '#b6927b'), ('copper', '#b98d7b'),
                ('amber', '#c4b28a'), ('sand', '#c8c093'),
                ('sage', '#87a987'), ('moss', '#8a9a7b'),
                ('teal', '#8ea4a2'), ('sea', '#7a8382')]
TONES = [(-.28, .78), (-.22, .88), (-.16, .98), (-.10, 1.0),
         (-.04, .90), (.02, .76), (.08, .56)]
UTILITIES = [('ink', '#0d0c0c'), ('charcoal', THEME['background']),
             ('graphite', '#393836'), ('gray', '#625e5a'), ('silver', '#a6a69c'),
             ('chalk', '#c5c9c5'), ('white', '#dcd7ba'), ('signal', '#ff9e3b')]
