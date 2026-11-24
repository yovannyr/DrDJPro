import unittest

from chord_pair_score import compare, triad


class ChordPairScoreTests(unittest.TestCase):
    def test_extensions_inversions_enharmonics_and_suspended_chords(self):
        self.assertEqual(triad("D#:min7/3"), "Eb:min")
        self.assertEqual(triad("F:9(*5)"), "F:maj")
        self.assertEqual(triad("C:maj7/1"), "C:maj")
        self.assertNotEqual(triad("A:sus4(7)"), triad("A:maj"))

    def test_boundaries_and_unsupported_chords_stay_in_reference(self):
        ref = [(0, 1, "D:min"), (1, 2, "G:min"), (2, 3, triad("A:sus4"))]
        pred = [(0, 1.1, "D:min"), (1.1, 2, "G:min"), (2, 3, "unknown")]
        result = compare(ref, pred)
        self.assertAlmostEqual(result["correct_s"], 1.9)
        self.assertEqual((result["seconds"], result["unknown_s"], result["unsupported_s"]), (3, 1, 1))
        self.assertAlmostEqual(result["f1"], 2 / 3)
        self.assertAlmostEqual(result["errors_s"][0], .1)
