"""Temporary CPU-only adapter to the original, isolated SheetSage2 pipeline."""
import json
import os
import re
import sys
import threading
import traceback
from pathlib import Path

ROOT = Path(sys.argv[1])
sys.path.insert(0, str(ROOT))
protocol = sys.stdout
protocol_lock = threading.RLock()


def emit(kind, **values):
    with protocol_lock:
        protocol.write(json.dumps({'type': kind, **values}, ensure_ascii=False) + '\n')
        protocol.flush()


class LogStream:
    def __init__(self):
        self.pending = ''

    def write(self, text):
        with protocol_lock:
            self.pending += str(text)
            while '\n' in self.pending:
                line, self.pending = self.pending.split('\n', 1)
                if line:
                    emit('log', message=line.rstrip('\r'))
                    match = re.search(r'\[进度\]\s*窗口\s+\d+/\d+.*?(\d+)%', line)
                    if match:
                        emit('progress', stage='CPU 识别窗口', fraction=int(match.group(1)) / 100)
                    elif '[步骤2/3]' in line:
                        emit('progress', stage='合并旋律和伴奏', fraction=None)
                    elif '[步骤3/3]' in line:
                        emit('progress', stage='导出 MIDI', fraction=None)
        return len(text)

    def flush(self):
        with protocol_lock:
            if self.pending:
                emit('log', message=self.pending)
                self.pending = ''


sys.stdout = LogStream()
os.environ['CUDA_VISIBLE_DEVICES'] = '-1'
os.environ['ROFORMER_DEVICE'] = 'cpu'
try:
    from transcriber import pipeline

    # Only the model child gets an isolated home: the existing pipeline can
    # still resolve user-configured paths such as ~/models normally. Its CLI
    # deletes two Path.home()/.cache names, so never give it the user's home.
    make_env = pipeline._make_env
    def isolated_env():
        env = make_env()
        env['USERPROFILE'] = str(ROOT / 'rust_app' / '.model_home')
        env['HOME'] = env['USERPROFILE']
        return env
    pipeline._make_env = isolated_env

    emit('progress', stage='CPU 识别中', fraction=None)
    pipeline.reset_cancel()
    result = pipeline.run_song_isolated(
        sys.argv[2], sys.argv[3], device='cpu', dtype='fp32',
        merge_mode='vocal', stems=sys.argv[4], sep_model='native',
        instrument_mode='acoustic', final_name=sys.argv[5] or None,
    )
    sys.stdout.flush()
    emit('completed', output=result['final_midi'])
except Exception as exc:
    sys.stdout.flush()
    emit('error', message=f'{type(exc).__name__}: {exc}')
    traceback.print_exc(file=sys.stderr)
    sys.exit(1)
