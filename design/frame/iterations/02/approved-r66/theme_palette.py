"""Four-accent orbital palette for Frame design only, never application runtime.
Source: https://github.com/rebelot/kanagawa.nvim/blob/master/lua/kanagawa/colors.lua
Cool orbital palette is Hex's own sci-fi variant; earlier Dragon trials are preserved.
"""
THEME = {
    'background': '#2a2828',
    'surface': '#2f2d2d',
    'well': '#242222',
    'overlay': '#363333',
    'separator': '#191717',
    'field_border': '#211e1e',
    'tag_border': '#211f1f',
    'tag_fill': '#242222',
    'selection_fill': '#35302f',
    'text': '#c3c5c0',
    'muted': '#9ea59f',
    'orange': '#f06449',
    'sea_glass': '#00e8df',
    'ice': '#79aebc',
    'blue': '#8faaba',
    'success': '#65ffad',
    'warning': '#ffd35e',
    'error': '#ff7086',
    'enabled': '#858f88',
    'waveform': '#c0c4bb',
    'piano_row': '#2e2b2b',
    'key_dark': '#625e5a',
    'key_light': '#a6a69c',
}
# Four vivid UI anchors; clip seeds preserve their hues at restrained lightness/chroma.
FAMILY_SEEDS = [('purple', '#a292d7'), ('blue', '#7ba0d8'),
                ('green', '#a4af46'), ('red', '#df7e86')]
SATURATION = {name: 1.0 for name, _ in FAMILY_SEEDS}
# Paired lightness/saturation choices; retain the core used in the views.
TONES = [(1, -.30, .35), (2, -.30, 1.40), (3, -.10, .35),
         (4, -.10, 1.0), (5, .08, 1.60), (6, .16, .35), (7, .20, .32)]
TONE_NAMES = {1: 'dark soft', 2: 'dark vivid', 3: 'mid soft', 4: 'core',
              5: 'bright vivid', 6: 'bright soft', 7: 'pale'}
UTILITIES = [('ink', '#10171e'), ('charcoal', THEME['background']),
             ('graphite', '#3b4651'), ('gray', '#657583'), ('silver', '#a2b2bf'),
             ('chalk', '#cad4dd'), ('white', '#edf3f8'), ('signal', '#ff8873')]
