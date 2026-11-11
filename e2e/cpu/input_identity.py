"""The real Source-to-grant path must share comparison bytes and isolate mutable copies."""

from pathlib import Path
import tempfile
import unittest
import wave

from cell import Cell, Json
from drive import Failed, InputHistory, inputs


class ComparisonInputs(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.calls = 0
        self.shared: InputHistory = {"echo/one": None}
        self.baseline: Cell = {"entry": "echo", "name": "one", "payload": {}, "files": {"audio": self.produce}}
        self.variant: Cell = {"entry": "echo", "name": "two", "payload": {}, "files": {"audio": self.produce},
                              "ranks": 2, "same_as": "echo/one"}

    def produce(self, folder: Path) -> Path:
        self.calls += 1
        path = folder / "tone.wav"
        with wave.open(str(path), "wb") as output:
            output.setnchannels(1)
            output.setsampwidth(2)
            output.setframerate(8000)
            output.writeframes((100 * self.calls).to_bytes(2, "little", signed=True) * 32)
        return path

    def test_a_fresh_encoding_is_not_used_for_a_comparison(self) -> None:
        first: dict[str, Json] = {}
        second: dict[str, Json] = {}
        meta1, grant1 = inputs(self.baseline, first, self.root / "inputs/one", self.root / "made", self.shared)
        meta2, grant2 = inputs(self.variant, second, self.root / "inputs/two", self.root / "made", self.shared)
        self.assertEqual(self.calls, 1)
        self.assertEqual(meta1, meta2)
        self.assertEqual(first, second)
        self.assertNotEqual(grant1["audio"]["local"], grant2["audio"]["local"])
        self.assertEqual(Path(grant1["audio"]["local"]).read_bytes(), Path(grant2["audio"]["local"]).read_bytes())

    def test_mutating_a_grant_does_not_mutate_its_twin(self) -> None:
        _, first = inputs(self.baseline, {}, self.root / "inputs/one", self.root / "made", self.shared)
        path = Path(first["audio"]["local"])
        expected = path.read_bytes()
        changed = bytearray(expected)
        changed[-1] ^= 1
        path.write_bytes(changed)
        _, second = inputs(self.variant, {}, self.root / "inputs/two", self.root / "made", self.shared)
        self.assertEqual(Path(second["audio"]["local"]).read_bytes(), expected)
        self.assertNotEqual(Path(second["audio"]["local"]).read_bytes(), path.read_bytes())

    def test_changed_snapshot_bytes_fail_before_model_preparation(self) -> None:
        inputs(self.baseline, {}, self.root / "inputs/one", self.root / "made", self.shared)
        snapshot = self.shared["echo/one"]
        assert snapshot is not None
        path = Path(snapshot["audio"]["local"])
        changed = bytearray(path.read_bytes())
        changed[-1] ^= 1
        path.write_bytes(changed)
        with self.assertRaisesRegex(Failed, "input bytes differ"):
            inputs(self.variant, {}, self.root / "inputs/two", self.root / "made", self.shared)


if __name__ == "__main__":
    unittest.main()
