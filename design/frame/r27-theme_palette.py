"""Kanagawa Dragon adaptation for Frame design only, never application runtime.
Source: https://github.com/rebelot/kanagawa.nvim/blob/master/lua/kanagawa/colors.lua
Core UI and clip seed values are upstream Dragon colors; tone variants are Hex's.
"""
THEME = {
    'background': '#181616',
    'surface': '#1d1c19',
    'well': '#12120f',
    'overlay': '#282727',
    'separator': '#393836',
    'field_border': '#282727',
    'text': '#c5c9c5',
    'muted': '#9e9b93',
    'orange': '#b6927b',
    'sea_glass': '#8ea4a2',
    'enabled': '#b98d7b',
    'waveform': '#282727',
    'piano_row': '#282727',
    'key_dark': '#625e5a',
    'key_light': '#a6a69c',
}
# Restrained warm-earth / green / aqua subset, preserving the narrower hue direction.
# Tone four is the exact upstream seed; other tones are derived in OKLCH.
FAMILY_SEEDS = [('clay', '#b6927b'), ('copper', '#b98d7b'),
                ('amber', '#c4b28a'), ('sand', '#c8c093'),
                ('sage', '#87a987'), ('moss', '#8a9a7b'),
                ('teal', '#8ea4a2'), ('sea', '#7a8382')]
TONES = [(-.22, .80), (-.15, .90), (-.08, 1.0), (0.0, 1.0),
         (.065, .88), (.12, .70), (.18, .45)]
UTILITIES = [('ink', '#0d0c0c'), ('charcoal', THEME['background']),
             ('graphite', '#393836'), ('gray', '#625e5a'), ('silver', '#a6a69c'),
             ('chalk', '#c5c9c5'), ('white', '#dcd7ba'), ('signal', '#ff9e3b')]
