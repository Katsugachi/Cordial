import unittest
from candidate_model_eval import numbers, pdf_screen, architecture_screen, english_screen


class CandidateScreenTests(unittest.TestCase):
    def test_numeric_screens(self):
        self.assertTrue(pdf_screen('Cone 75.40 cm²; rod fits, diagonal 47.17 cm; cube 98.534 cm³'))
        self.assertFalse(pdf_screen('Cone 75.40; rod does not fit, 47.17; cube 98.534'))
        self.assertFalse(pdf_screen('Cone 75.40; rod fits, 47.17; cube 512'))
        self.assertTrue(architecture_screen('Area 336, perimeter 74, packs 148, cost $7,104'))
        self.assertFalse(architecture_screen('336 74 148 71040'))
        self.assertEqual(numbers('cost 7,104.50 and 74'), [7104.5, 74.0])

    def test_english(self):
        self.assertTrue(english_screen('Personification and metaphor. Shakespeare, 1609.'))
        self.assertFalse(english_screen('Personification and metaphor. Shakespeare, 1610.'))
        self.assertFalse(english_screen('Personification and metaphor, Shakespeare, 1609 First Folio.'))
        self.assertFalse(english_screen('Personification and metaphor, Shakespeare, 1609. Implied simile.'))


if __name__ == '__main__':
    unittest.main()
