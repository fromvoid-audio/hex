"""Kanagawa Dragon adaptation for Frame design only, never application runtime.
Source: https://github.com/rebelot/kanagawa.nvim/blob/master/lua/kanagawa/colors.lua
Cool orbital palette is Hex's own sci-fi variant; earlier Dragon trials are preserved.
"""
THEME = {
    'background': '#2b2d2d',
    'surface': '#303333',
    'well': '#242827',
    'overlay': '#343b38',
    'separator': '#1c2020',
    'field_border': '#232827',
    'tag_border': '#535d56',
    'text': '#bfc5bd',
    'muted': '#a3aca2',
    'orange': '#f0ab73',
    'sea_glass': '#70c9bd',
    'enabled': '#8ba39b',
    'waveform': '#282727',
    'piano_row': '#2c302e',
    'key_dark': '#625e5a',
    'key_light': '#a6a69c',
}
# Limited ember / ice / mint / cyan groups, with cool hues doing most of the work.
FAMILY_SEEDS = [('ember', '#cf9570'), ('copper', '#c39488'),
                ('ice', '#88a7bf'), ('silver', '#94a8bb'),
                ('mint', '#79b89f'), ('jade', '#75aca4'),
                ('teal', '#72b0b3'), ('cyan', '#7eb8c9')]
SATURATION = {'ember': 1.0, 'copper': .85, 'ice': 1.15, 'silver': .85,
              'mint': 1.10, 'jade': 1.10, 'teal': 1.05, 'cyan': 1.10}
TONES = [(-.28, .78), (-.22, .88), (-.16, .98), (-.10, 1.0),
         (-.04, .90), (.02, .76), (.08, .56)]
UTILITIES = [('ink', '#10171e'), ('charcoal', THEME['background']),
             ('graphite', '#3b4651'), ('gray', '#657583'), ('silver', '#a2b2bf'),
             ('chalk', '#cad4dd'), ('white', '#edf3f8'), ('signal', '#f0ab73')]
