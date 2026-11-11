"""The Python checkpoint constructor bridge matches its canonical source getter."""
import importlib.util
from pathlib import Path
import re
import unittest


class CheckpointStubTests(unittest.TestCase):
    def test_checkpoint_inputs_accept_the_finite_identity_bridge(self) -> None:
        script = Path(__file__).with_name("pyi.py")
        spec = importlib.util.spec_from_file_location("executor_stub", script)
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        text = module.render(script.parent.parent.joinpath("src/lib.rs").read_text())
        fields_match = re.search(r'_CheckpointItemFields = .*', text)
        changes_match = re.search(r'_CheckpointItemChanges = .*', text)
        assert fields_match is not None and changes_match is not None
        fields = fields_match.group()
        changes = changes_match.group()
        self.assertIn("'model_id': NotRequired[str]", fields)
        self.assertIn("'repo_id': NotRequired[str]", fields)
        self.assertIn("'model_id': str", changes)
        self.assertIn("'repo_id': str", changes)
        checkpoint = text.split("class CheckpointItem:", 1)[1].split("@final", 1)[0]
        self.assertIn("def model_id(self) -> str:", checkpoint)
        self.assertNotIn("def repo_id(self)", checkpoint)


if __name__ == "__main__":
    unittest.main()
