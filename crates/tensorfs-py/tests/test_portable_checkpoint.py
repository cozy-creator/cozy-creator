"""Portable checkpoints through the public Python Writer/open API, without frameworks."""

from array import array
import gc
import json
from pathlib import Path
import shutil
import tempfile
import unittest

import tensorfs


class PortableCheckpointTests(unittest.TestCase):
    def test_writer_destination_is_complete_in_a_fresh_offline_store(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source-store"
            artifact = root / "model.cozytensors"
            with tensorfs.Writer(artifact, cache_dir=source) as writer:
                written_component = writer.component(
                    "model", cls=("fixture", "1", "Linear"),
                    config={"in_features": 2, "out_features": 2},
                )
                written_component.put("weight", array("f", [1, 2, 3, 4]), dtype="F32", shape=[2, 2])
                written_component.asset("tokenizer.json", json.dumps({"vocab": {"hello": 2, "world": 3}}).encode())
            saved = writer.commit()
            header_id = saved.id
            self.assertGreater(artifact.stat().st_size, 8192)
            saved.close()
            del saved, written_component, writer
            gc.collect()
            shutil.rmtree(source)

            checkpoint = tensorfs.open(artifact, cache_dir=root / "fresh-store", local_files_only=True)
            self.assertEqual(checkpoint.id, header_id)
            self.assertEqual(checkpoint.missing(), [])
            model = checkpoint.components["model"]
            self.assertEqual(model.config["in_features"], 2)
            values = array("f", model.tensors.read("weight", decode=True).tobytes()).tolist()
            vocab = json.loads(model.asset("tokenizer.json"))["vocab"]
            tokens = [vocab[word] for word in "hello world".split()]
            self.assertEqual([values[0] * tokens[0] + values[1] * tokens[1],
                              values[2] * tokens[0] + values[3] * tokens[1]], [8.0, 18.0])
            copied = root / "copied.cozytensors"
            checkpoint.write_file(copied)
            self.assertEqual(artifact.read_bytes(), copied.read_bytes())
            with self.assertRaisesRegex(tensorfs.NotFoundError, "missing-tokenizer"):
                model.asset("missing-tokenizer.model")
            checkpoint.close()


if __name__ == "__main__":
    unittest.main()
