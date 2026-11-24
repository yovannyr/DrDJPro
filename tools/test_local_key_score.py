import unittest

from local_key_score import boundary_errors, camelot, checked, score


class LocalKeyScoreTests(unittest.TestCase):
    def test_enharmonic_notes_preserve_mode(self):
        self.assertEqual(camelot("A#:maj"), camelot("Bb:maj"))
        self.assertEqual((camelot("C:maj"), camelot("D:maj"), camelot("A:min")),
                         ("8B", "10B", "8A"))

    def test_boundary_matching_uses_nearest_of_equally_many_matches(self):
        self.assertEqual(boundary_errors([1, 3], [2.75], 2), [0.25])
        self.assertEqual(boundary_errors([1, 3], [2, 4], 1), [1, 1])
        self.assertEqual(boundary_errors([1, 2], [4], 1), [])

    def test_empty_prediction_gaps_and_unknowns_remain_in_denominator(self):
        ref = [(1, 5, "8B"), (5, 9, "10B")]
        result = score(ref, [(0, 2, "8B"), (3, 6, "unknown"), (6, 10, "10B")], [])
        self.assertEqual((result["seconds"], result["correct_s"], result["unknown_s"]), (8, 4, 4))
        self.assertEqual((result["best_constant_s"], result["missed"], result["unknown_boundaries"]),
                         (4, 1, 1))

    def test_chord_change_away_from_modulation_is_reported_separately(self):
        ref = [(0, 6, "8B"), (6, 12, "10B")]
        estimate = [(0, 2, "8B"), (2, 6, "7B"), (6, 12, "10B")]
        chords = [(0, 2, "C:maj"), (2, 4, "F:maj"), (4, 12, "D:maj")]
        result = score(ref, estimate, chords)
        self.assertEqual((result["matched"], result["extra"], result["near_chord_only"]), (1, 1, 1))
        with self.assertRaises(ValueError):
            checked([(0, 2, "8B"), (1, 3, "8B")])
