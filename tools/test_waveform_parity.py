"""The native gate requires an exact image and geometry match for each view."""
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

from qualify_waveform_parity import digest, verify


class NativeOutputs(unittest.TestCase):
    def test_changed_and_missing_files_are_reported_independently(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            group = root / "control"
            group.mkdir()
            png = group / "overview.png"
            geometry = group / "overview.json"
            png.write_bytes(b"accepted image")
            geometry.write_bytes(b"accepted geometry")
            views = {"control": [{"id": "overview", "imageSha256": digest(png),
                                  "geometrySha256": digest(geometry)}]}
            self.assertEqual(verify(root, views), [])
            png.write_bytes(b"changed image")
            self.assertEqual(verify(root, views), ["control/overview.png"])
            geometry.unlink()
            self.assertEqual(verify(root, views), ["control/overview.png", "control/overview.json"])


if __name__ == "__main__":
    unittest.main()
