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
    def test_model_identity_keyword_bridge_is_finite_and_unambiguous(self):
        from tensorfs import _ext
        for writer in (tensorfs.Writer, _ext.Writer):
            with self.assertRaisesRegex(TypeError, "cannot both be supplied"):
                writer(model_id="local/example", repo_id="local/example")
        with self.assertRaisesRegex(TypeError, "cannot both be supplied"):
            tensorfs.download_file(model_id="local/example", repo_id="local/example", filename="README.md")
        with self.assertRaisesRegex(TypeError, "cannot both be supplied"):
            tensorfs.upload_folder(model_id="local/example", repo_id="local/example", folder_path=".", lane="f32")
        with tempfile.TemporaryDirectory() as directory:
            for keyword in ("model_id", "repo_id"):
                writer = tensorfs.Writer(cache_dir=directory, lane="f32", **{keyword: "local/example"})
                component = writer.component("model", config={"width": 1})
                component.put("weight", array("f", [2]), dtype="F32", shape=[1])
                checkpoint = writer.commit()
                self.assertEqual(checkpoint.model_id, "local/example")
                self.assertEqual(checkpoint.repo_id, checkpoint.model_id)
                checkpoint.close()

    def test_executor_checkpoint_identity_preserves_the_supported_frame(self):
        from tensorfs.executor import CheckpointItem
        fields = {"output": "weights", "revision": "sha256:" + "0" * 64}
        canonical = CheckpointItem(model_id="local/example", **fields)
        legacy = CheckpointItem(repo_id="local/example", **fields)
        self.assertEqual(canonical, legacy)
        self.assertEqual(canonical.model_id, "local/example")
        frame = json.loads(canonical.json())
        self.assertEqual(frame["repo_id"], "local/example")
        self.assertNotIn("model_id", frame)
        self.assertEqual(canonical.replace(model_id="local/replaced").model_id, "local/replaced")
        with self.assertRaises((TypeError, ValueError)):
            CheckpointItem(model_id="local/example", repo_id="local/example", **fields)

    def test_writer_destination_is_complete_in_a_fresh_offline_store(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source-store"
            artifact = root / "model.cozytensors"
            with tensorfs.Writer(artifact, cache_dir=source) as writer:
                model = writer.component(
                    "model", cls=("fixture", "1", "Linear"),
                    config={"in_features": 2, "out_features": 2},
                )
                model.put("weight", array("f", [1, 2, 3, 4]), dtype="F32", shape=[2, 2])
                model.asset("tokenizer.json", json.dumps({"vocab": {"hello": 2, "world": 3}}).encode())
            saved = writer.commit()
            header_id = saved.id
            self.assertGreater(artifact.stat().st_size, 8192)
            saved.close()
            del saved, model, writer
            gc.collect()
            shutil.rmtree(source)

            checkpoint = tensorfs.open(artifact, cache_dir=root / "fresh-store", local_files_only=True)
            self.assertEqual(checkpoint.id, header_id)
            self.assertEqual(checkpoint.missing(), [])
            model = checkpoint.components["model"]
            self.assertEqual(model.config["in_features"], 2)
            values = array("f", model.tensors.read("weight").tobytes()).tolist()
            vocab = json.loads(model.asset("tokenizer.json"))["vocab"]
            tokens = [vocab[word] for word in "hello world".split()]
            self.assertEqual([values[0] * tokens[0] + values[1] * tokens[1],
                              values[2] * tokens[0] + values[3] * tokens[1]], [8.0, 18.0])
            copied = root / "copied.cozytensors"
            checkpoint.write_file(copied)
            self.assertEqual(artifact.read_bytes(), copied.read_bytes())
            with self.assertRaisesRegex(tensorfs.InvalidError, "missing-tokenizer"):
                model.asset("missing-tokenizer.model")
            checkpoint.close()


if __name__ == "__main__":
    unittest.main()
