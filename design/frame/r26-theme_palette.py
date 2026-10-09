"""Shared palette: graphite, ember and sea glass; never application runtime."""
THEME = {
    'background': '#242526',
    'surface': '#2a2b2c',
    'well': '#202223',
    'overlay': '#343738',
    'separator': '#3b3e3f',
    'field_border': '#303334',
    'text': '#d5d8d5',
    'muted': '#a8b0ad',
    'orange': '#ff9955',
    'sea_glass': '#78d4b1',
    'enabled': '#b58d70',
    'waveform': '#303635',
    'piano_row': '#2e3132',
    'key_dark': '#515c59',
    'key_light': '#99aaa3',
}
# Four hue anchors, each with a softer companion: warm earth and botanical tints.
# No blue, violet, magenta or red families; tonal variation supplies the range.
FAMILY_SPECS = [('clay', 48, 1.0), ('copper', 48, .65),
                ('amber', 80, .85), ('sand', 80, .50),
                ('sage', 145, .85), ('moss', 145, .50),
                ('teal', 175, 1.0), ('sea', 175, .65)]
TONES = [(0.43, .040), (0.49, .050), (0.55, .050), (0.62, .055),
         (0.69, .052), (0.76, .045), (0.83, .038)]
UTILITIES = [('ink', '#152123'), ('charcoal', THEME['background']),
             ('graphite', '#465452'), ('gray', '#7e8f89'), ('silver', '#b3c3bb'),
             ('chalk', '#dce5dd'), ('white', '#f3f6ee'), ('signal', '#ff9955')]
