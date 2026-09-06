"""Safety checks for one-way source/build synchronization; all data is synthetic."""
from pathlib import Path
import tempfile
import unittest
from sync_workspace import sync, MARKER


class SyncTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.source = Path(self.temp.name) / 'source'
        self.build = Path(self.temp.name) / 'build'
        self.source.mkdir()
        (self.source / 'Cargo.toml').write_text('[workspace]\n')
        (self.source / 'main.rs').write_text('fn main() {}\n')

    def test_preserves_private_data_and_removes_only_managed_files(self):
        (self.source / '.env').write_text('LOCAL_VALUE=private\n')
        sync(self.source, self.build)
        self.assertFalse((self.build / '.env').exists())
        (self.build / '.env').write_text('LOCAL_VALUE=private\n')
        (self.build / 'target').mkdir()
        (self.build / 'target/cache').write_text('cache')
        (self.build / 'notes.txt').write_text('local notes')
        (self.source / 'main.rs').unlink()
        sync(self.source, self.build)
        self.assertFalse((self.build / 'main.rs').exists())
        self.assertEqual((self.build / '.env').read_text(), 'LOCAL_VALUE=private\n')
        self.assertEqual((self.build / 'target/cache').read_text(), 'cache')
        self.assertTrue((self.build / 'notes.txt').exists())

    def test_conflict_preflights_all_changes(self):
        sync(self.source, self.build)
        (self.build / 'main.rs').write_text('local edit')
        (self.source / 'Cargo.toml').write_text('[workspace]\nmembers=[]\n')
        with self.assertRaisesRegex(ValueError, 'Local edit'):
            sync(self.source, self.build)
        self.assertEqual((self.build / 'Cargo.toml').read_text(), '[workspace]\n')
        (self.source / 'main.rs').unlink()
        with self.assertRaises(ValueError):
            sync(self.source, self.build)

    def test_dry_run_and_no_reverse_or_nested_copy(self):
        self.assertEqual(sync(self.source, self.build, True)[1], 2)
        self.assertFalse(self.build.exists())
        with self.assertRaises(ValueError):
            sync(self.source, self.source / 'build')
        sync(self.source, self.build)
        self.assertEqual(sync(self.source, self.build, True)[1:], (0, 0))
        with self.assertRaises(ValueError):
            sync(self.build, self.source)

    def test_nonempty_destination_and_symlink_are_rejected(self):
        self.build.mkdir()
        (self.build / 'user.txt').write_text('keep')
        with self.assertRaises(ValueError):
            sync(self.source, self.build)
        try:
            (self.source / 'linked.rs').symlink_to(self.source / 'main.rs')
        except OSError:
            return  # Windows developer-mode/admin privileges may be unavailable.
        with self.assertRaises(ValueError):
            sync(self.source, Path(self.temp.name) / 'other')

    def test_interrupted_sync_can_retry(self):
        sync(self.source, self.build)
        (self.source / 'main.rs').write_text('updated source')
        # File write succeeded before a process died and updated the manifest.
        (self.build / 'main.rs').write_text('updated source')
        sync(self.source, self.build)
        self.assertEqual(sync(self.source, self.build, True)[1:], (0, 0))


if __name__ == '__main__':
    unittest.main()
