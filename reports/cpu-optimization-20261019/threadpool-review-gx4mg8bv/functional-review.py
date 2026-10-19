"""Compare the same isolated app with/without Qt GUI workers; no real captures."""
from pathlib import Path
import hashlib, importlib.util, json, os, random, struct, subprocess, time, zlib

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('private_mcp', ROOT / 'mcp-client.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

def fixture(path, width=1200, height=800):
    randomizer = random.Random(20261017)
    def chunk(stream, kind, data):
        stream.write(struct.pack('>I', len(data)) + kind + data +
                     struct.pack('>I', zlib.crc32(kind + data) & 0xffffffff))
    with path.open('wb') as stream:
        stream.write(b'\x89PNG\r\n\x1a\n')
        chunk(stream, b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0))
        encoded = zlib.compressobj(1)
        for _ in range(height):
            chunk(stream, b'IDAT', encoded.compress(b'\0' + randomizer.randbytes(width * 3)))
        chunk(stream, b'IDAT', encoded.flush())
        chunk(stream, b'IEND', b'')

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def run(mode):
    folder = ROOT / ('functional-' + mode)
    folder.mkdir(mode=0o700, exist_ok=True)
    data = folder / 'data'
    data.mkdir(mode=0o700, exist_ok=True)
    config = json.loads((ROOT / 'baseline-data/config.json').read_text())
    config.setdefault('mcp', {})['enabled'] = True
    (data / 'config.json').write_text(json.dumps(config))
    (data / 'config.json').chmod(0o600)
    descriptor = folder / 'server.json'
    descriptor.unlink(missing_ok=True)
    env = os.environ.copy()
    env.pop('QT_NO_GUI_THREADPOOL', None)
    if mode == 'disabled':
        env['QT_NO_GUI_THREADPOOL'] = '1'
    env.update(SNOW_SHOT_PERSONAL_DATA_DIR=str(data), SNOW_SHOT_STATIC_BACKDROP='0',
               SNOW_SHOT_MCP_DESCRIPTOR=str(descriptor), QT_LOGGING_RULES='',
               LC_ALL='en_US.UTF-8')
    app = Path(json.loads((ROOT / 'metadata.json').read_text())['app']) / 'Contents/MacOS/snow_shot'
    log = (folder / 'app.log').open('w')
    process = subprocess.Popen([str(app), '--e2e-instance-id=functional-' + mode],
                               env=env, stdout=log, stderr=subprocess.STDOUT)
    client = None
    results = {'mode': mode, 'app_sha256': digest(app), 'calls': [], 'files': {}}
    def call(method, **params):
        started = time.monotonic()
        response = client.request('snow_shot_' + method, params)
        results['calls'].append({'method': method, 'elapsed_ms': (time.monotonic()-started)*1000})
        return response
    try:
        deadline = time.monotonic() + 30
        while not descriptor.exists():
            if process.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError('Isolated app did not start')
            time.sleep(.1)
        client = module.Client(descriptor, process.pid)
        opened = call('document_open', path=str(ROOT / 'functional-source.png'))
        document_id = opened['result']['document_id']
        def doc(suffix, **params):
            return call('document_' + suffix, document_id=document_id, **params)
        def save(name, **params):
            path = folder / name
            response = doc('save', path=str(path), **params)
            results['files'][name] = {'sha256': digest(path), 'bytes': path.stat().st_size,
                                      'width': response['result'].get('width'),
                                      'height': response['result'].get('height')}
            return digest(path)
        doc('set_selection_style', corner_radius=0, shadow_width=0)
        original = save('original.png')
        doc('apply_annotations', operations=[
            {'type': 'rectangle', 'bounds': [40, 40, 300, 220],
             'style': {'stroke': [255, 0, 0, 255], 'stroke_width': 6}},
            {'type': 'arrow', 'points': [[100, 300], [650, 480]],
             'style': {'stroke': [0, 50, 255, 255], 'stroke_width': 9}},
            {'type': 'text', 'bounds': [400, 80, 600, 120], 'text': 'Snow Shot / 中文 123',
             'style': {'font_size': 32}},
            {'type': 'rectangle_filter', 'bounds': [600, 550, 280, 150]},
            {'type': 'rectangle_highlight', 'bounds': [80, 570, 300, 100]},
        ])
        annotated = save('annotated.png')
        assert annotated != original, 'Annotations changed no pixels'
        doc('undo')
        assert save('undo.png') == original, 'Undo did not restore the original output'
        doc('redo')
        assert save('redo.png') == annotated, 'Redo did not restore the annotations'
        doc('set_selection', type='rectangle', bounds=[20, 20, 1000, 700])
        save('crop.png')
        save('scale-half.png', scale=.5)
        save('scale-one-half.png', scale=1.5)
        save('export.jpg', format='jpeg')
        doc('set_selection_style', corner_radius=16, shadow_width=12)
        save('rounded-shadow.png')
        doc('set_selection_style', corner_radius=0, shadow_width=0)
        revision = client.revision
        existing = {pin['id'] for pin in call('pinned_list')['result']['windows']}
        client.revision = revision
        doc('pin')
        pin_id = next(pin['id'] for pin in call('pinned_list')['result']['windows']
                      if pin['id'] not in existing)
        def pin(suffix, **params):
            for _ in range(5):
                call('pinned_get', id=pin_id)
                try:
                    return call('pinned_' + suffix, id=pin_id, **params)
                except RuntimeError as error:
                    if 'stale_revision' not in str(error):
                        raise
                    time.sleep(.05)
            raise RuntimeError('Pin revision kept changing')
        assert call('pinned_get', id=pin_id)['result']['can_undo']
        pin('edit', action='undo')
        assert call('pinned_get', id=pin_id)['result']['can_redo']
        pin('edit', action='redo')
        pin('update', properties={'opacity_percent': 70, 'rotation': 'clockwise',
                                  'show_border': False})
        for name, is_original in [('pin-rotated.png', False), ('pin-original.png', True)]:
            path = folder / name
            pin('export', output='save', original=is_original, path=str(path))
            dimensions = struct.unpack('>II', path.read_bytes()[16:24])
            assert dimensions == ((1000, 700) if is_original else (700, 1000)), dimensions
            results['files'][name] = {'sha256': digest(path), 'bytes': path.stat().st_size,
                                      'width': dimensions[0], 'height': dimensions[1]}
        pin('action', action='destroy')
        doc('state')
        doc('close')
        results['passed'] = True
        return results
    finally:
        (folder / 'results.json').write_text(json.dumps(results, indent=2))
        if client:
            try:
                client.request('snow_shot_app_action', {'action': 'quit'})
            except Exception:
                pass
            client.socket.close()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.terminate()
            process.wait(timeout=5)
        log.close()

if __name__ == '__main__':
    fixture(ROOT / 'functional-source.png')
    results = [run(mode) for mode in ['default', 'disabled']]
    differences = [name for name in results[0]['files']
                   if results[0]['files'][name]['sha256'] != results[1]['files'][name]['sha256']]
    report = {'runs': results, 'different_outputs': differences,
              'all_outputs_byte_identical': not differences}
    (ROOT / 'functional-comparison.json').write_text(json.dumps(report, indent=2))
    print(json.dumps({'passed': all(result.get('passed') for result in results),
                      'outputs_per_mode': len(results[0]['files']), 'differences': differences}))
    assert not differences, 'Threadpool modes produced different encoded outputs'
