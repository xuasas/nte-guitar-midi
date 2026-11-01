import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from transcriber import pipeline
from transcriber.convert import Note


class CpuPipelineTests(unittest.TestCase):
    def test_cpu_even_for_old_cuda_settings(self):
        self.assertEqual(pipeline.resolve_device('cuda'), 'cpu')
        self.assertEqual(pipeline.resolve_dtype('cpu', 'bf16'), 'fp32')
        env = pipeline._make_env()
        self.assertEqual(env['CUDA_VISIBLE_DEVICES'], '-1')
        self.assertNotIn('PYTHONPATH', env)

    def test_vocal_priority_with_instrument_gaps(self):
        vocal = [Note(72, 1.0, 2.0, 70)]
        instruments = [Note(60, 0.9, 2.1, 100), Note(62, 2.2, 2.6, 90)]
        notes = pipeline.merge_melodies(vocal, instruments, merge_mode='vocal')
        self.assertEqual([n.pitch for n in notes], [72, 62])

    def test_output_does_not_replace_existing_song(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            (out / 'song.mid').write_bytes(b'original')
            def fake_run(_audio, work, **_kwargs):
                result = Path(work) / 'song.mid'
                result.write_bytes(b'new')
                return {'final_midi': str(result), 'steps': [{'outputs': {}}]}
            with patch.object(pipeline, 'run_all', side_effect=fake_run):
                result = pipeline.run_song_isolated('song.wav', out)
            self.assertEqual((out / 'song.mid').read_bytes(), b'original')
            self.assertEqual(Path(result['final_midi']).read_bytes(), b'new')


if __name__ == '__main__':
    unittest.main()
