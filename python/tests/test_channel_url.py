import unittest

from mast_pager import Channel, parse_channel_url

KEY = "mk_" + "1c9f" * 10


class ParseChannelUrl(unittest.TestCase):
    def test_short_form(self):
        self.assertEqual(
            parse_channel_url("https://mast.tissue.dev/" + KEY),
            ("https://mast.tissue.dev", "", KEY),
        )

    def test_ingest_path(self):
        origin, prefix, key = parse_channel_url("https://api.tissue.systems/m/" + KEY)
        self.assertEqual((origin, prefix, key), ("https://api.tissue.systems", "/m", KEY))

    def test_bare_key(self):
        self.assertEqual(parse_channel_url(KEY), ("https://mast.tissue.dev", "", KEY))

    def test_host_without_a_scheme(self):
        origin, _, _ = parse_channel_url("mast.tissue.dev/" + KEY)
        self.assertEqual(origin, "https://mast.tissue.dev")

    def test_surrounding_whitespace(self):
        # Keys get pasted out of the app, and a copied line brings a newline.
        self.assertEqual(parse_channel_url("  " + KEY + "\n")[2], KEY)

    def test_the_pasted_prefix_is_kept(self):
        # mast.tissue.dev rewrites / onto /m/, but a self-hosted edge need not,
        # so both spellings have to keep reaching the route they were given.
        self.assertEqual(Channel("https://example.test/m/" + KEY).url,
                         "https://example.test/m/" + KEY)
        self.assertEqual(Channel("https://example.test/" + KEY).url,
                         "https://example.test/" + KEY)

    def test_a_port_survives(self):
        origin, _, _ = parse_channel_url("http://127.0.0.1:8082/m/" + KEY)
        self.assertEqual(origin, "http://127.0.0.1:8082")

    def test_empty(self):
        with self.assertRaises(ValueError):
            parse_channel_url("")

    def test_no_key_in_the_url(self):
        with self.assertRaises(ValueError):
            parse_channel_url("https://mast.tissue.dev/")

    def test_wrong_shape_of_key(self):
        with self.assertRaises(ValueError):
            parse_channel_url("https://mast.tissue.dev/mk_nothex")

    def test_uppercase_hex_is_not_a_key(self):
        with self.assertRaises(ValueError):
            parse_channel_url("mk_" + "1C9F" * 10)

    def test_not_http(self):
        with self.assertRaises(ValueError):
            parse_channel_url("ftp://mast.tissue.dev/" + KEY)

    def test_repr_does_not_carry_the_key(self):
        text = repr(Channel(KEY))
        self.assertNotIn(KEY, text)
        self.assertIn(KEY[:7], text)


if __name__ == "__main__":
    unittest.main()
