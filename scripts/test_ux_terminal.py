import unittest
from ux_terminal import screen_text


class TerminalScreenTests(unittest.TestCase):
    def test_status_repaint_replaces_stale_message(self):
        raw = b'\x1b[2J\x1b[4;1Hsaved fixture.md\x1b[4;1Hsave \x1b[31mcancelled\x1b[K'
        self.assertEqual(screen_text(raw, 40, 4).splitlines()[-1], 'save cancelled')

    def test_patch_keeps_unchanged_letters_between_cursor_moves(self):
        raw = b'\x1b[2;1Hsave cXncelled\x1b[2;7Ha'
        self.assertEqual(screen_text(raw, 30, 2).splitlines()[-1], 'save cancelled')

    def test_protocol_and_clipboard_are_not_rendered_text(self):
        raw = b'\x1b[?2004h\x1b]52;c;Y2xpcGJvYXJk\x07visible\x1b[?25l'
        self.assertEqual(screen_text(raw, 20, 1), 'visible')

    def test_clear_line_and_cursor_relative(self):
        raw = b'obsolete\x1b[1Gnew\x1b[K\x1b[2B\x1b[1Glast'
        self.assertEqual(screen_text(raw, 20, 3), 'new\n\nlast')

    def test_unicode_columns_and_combining_characters(self):
        raw = '漢e\u0301'.encode() + b'\x1b[1;4H!'
        self.assertEqual(screen_text(raw, 10, 1), '漢e\u0301!')

    def test_incomplete_escape_is_not_a_status(self):
        self.assertEqual(screen_text(b'saved\x1b[', 20, 1), 'saved')


if __name__ == '__main__':
    unittest.main()
