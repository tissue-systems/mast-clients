import ast
import importlib.util
import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent / "tools"))
import vendor  # noqa: E402

from .fake_mast import MESSAGE_ID, FakeMast  # noqa: E402


def load_vendored():
    spec = importlib.util.spec_from_file_location("mast_vendored", vendor.TARGET)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class VendoredCopy(unittest.TestCase):
    def test_it_is_in_sync_with_the_package(self):
        self.assertEqual(vendor.TARGET.read_text(), vendor.render(),
                         "run python tools/vendor.py")

    def test_it_imports_nothing_of_its_own(self):
        # The copy is one file, so a relative import in the client would break
        # it the moment somebody pastes it into their tree.
        source = vendor.SOURCE.read_text()
        self.assertNotIn("\nfrom .", source)
        self.assertNotIn("\nimport mast_pager", source)

    def test_it_works_on_its_own(self):
        mast = FakeMast().start()
        self.addCleanup(mast.stop)
        module = load_vendored()
        sent = module.Channel(mast.url).send(title="db-01", body="down")
        self.assertEqual(sent.id, MESSAGE_ID)

    def test_the_standard_library_is_all_it_needs(self):
        tree = ast.parse(vendor.TARGET.read_text())
        imported = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                imported.update(alias.name.split(".")[0] for alias in node.names)
            elif isinstance(node, ast.ImportFrom):
                imported.add((node.module or "").split(".")[0])
        self.assertTrue(imported)
        self.assertTrue(imported <= sys.stdlib_module_names, imported)


if __name__ == "__main__":
    unittest.main()
