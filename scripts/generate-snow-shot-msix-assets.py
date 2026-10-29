"""Rasterize the existing SVG; requires CairoSVG only when regenerating committed assets."""

import hashlib
import json
from pathlib import Path

import cairosvg


def main():
    root = Path(__file__).resolve().parents[1]
    source = root / 'snow_shot/resources/app-icon.svg'
    output = root / 'snow_shot/packaging/msix/Assets'
    output.mkdir(parents=True, exist_ok=True)
    sizes = {'StoreLogo': 50, 'Square44x44Logo': 44, 'Square150x150Logo': 150}
    for name, size in sizes.items():
        for scale in (100, 200, 400):
            filename = f'{name}.png' if scale == 100 else f'{name}.scale-{scale}.png'
            cairosvg.svg2png(url=str(source), write_to=str(output / filename),
                            output_width=size * scale // 100, output_height=size * scale // 100)
    for size in (16, 24, 32, 48, 256):
        cairosvg.svg2png(url=str(source),
                        write_to=str(output / f'Square44x44Logo.targetsize-{size}_altform-unplated.png'),
                        output_width=size, output_height=size)
    (output / 'source.json').write_text(json.dumps({
        'source': 'snow_shot/resources/app-icon.svg',
        'sha256': hashlib.sha256(source.read_bytes()).hexdigest(),
    }, indent=2) + '\n', encoding='utf-8', newline='\n')


if __name__ == '__main__':
    main()
