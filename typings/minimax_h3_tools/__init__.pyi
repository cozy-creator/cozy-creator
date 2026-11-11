# The root stand-in e2e/isolated.sh publishes as minimax-h3-tools (release-gate/suites/hub/pkg), as e2e/cli/relay calls it.
import msgspec
from cozy_runtime.author import App

RELEASE: str
app: App

class Text(msgspec.Struct, forbid_unknown_fields=True):
    text: str

class Echoed(msgspec.Struct):
    text: str
    release: str

def echo(payload: Text) -> Echoed: ...
