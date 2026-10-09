"""Check the actual release screenshot, including sides beside scrolling canvases."""
import json
import math
from pathlib import Path
from PIL import Image

root = Path(__file__).resolve().parents[1] / 'docs' / 'validation'
report = json.loads((root / 'panel-layout-ui.json').read_text(encoding='utf-8'))
image = Image.open(root / 'panel-layout-all.png').convert('RGB')
color = (81, 85, 92)
results = []
for panel in report['allPanels']:
    box = panel['rect']
    result = {'panel': panel['id']}
    for side in ('left', 'right', 'top', 'bottom'):
        counts = []
        for fraction in (.05, .25, .5, .75, .95):
            x = math.floor(box['left'] + box['width'] * fraction)
            y = math.floor(box['top'] + box['height'] * fraction)
            edge = math.floor(box[side])
            vertical = side in ('left', 'right')
            count = sum(image.getpixel((i, y) if vertical else (x, i)) == color
                        for i in range(edge - 3, edge + 4)
                        if 0 <= i < (image.width if vertical else image.height))
            assert count == 2, (panel['id'], side, fraction, count)
            counts.append(count)
        result[side] = counts
    results.append(result)
(root / 'panel-layout-border-pixels.json').write_text(json.dumps(results, indent=2), encoding='utf-8')
print('PASS: 5 panels x 4 sides x 5 samples = 100 visible two-pixel edges')
