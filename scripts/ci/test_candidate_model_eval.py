import io
import unittest
from unittest.mock import patch
from candidate_model_eval import numbers, pdf_screen, architecture_screen, english_screen, request, python_pdf_screen


class CandidateScreenTests(unittest.TestCase):
    def test_numeric_screens(self):
        self.assertTrue(pdf_screen('Cone 75.40 cm²; rod fits, diagonal 47.17 cm; cube 98.534 cm³'))
        self.assertFalse(pdf_screen('Cone 75.40; rod does not fit, 47.17; cube 98.534'))
        self.assertFalse(pdf_screen('Cone 75.40; rod fits, 47.17; cube 512'))
        self.assertFalse(pdf_screen('Cone 75.40; rod fits, 47.17; cube 98.538'))
        self.assertTrue(architecture_screen('Area 336, perimeter 74, packs 148, cost $7,104'))
        self.assertFalse(architecture_screen('336 74 148 71040'))
        self.assertEqual(numbers('cost 7,104.50 and 74'), [7104.5, 74.0])

    def test_truncated_generation_remains_available_for_diagnostics(self):
        payload = b'{"choices":[{"finish_reason":"length","message":{"content":"partial"}}]}'
        with patch("urllib.request.urlopen", return_value=io.BytesIO(payload)):
            self.assertEqual(request([], 1), ("partial", "length"))

    def test_python_screen_uses_returned_tool_result(self):
        replies = [
            {'finish_reason': 'tool_calls', 'message': {'role': 'assistant', 'tool_calls': [{'id': 'a', 'type': 'function', 'function': {'name': 'python_calculate', 'arguments': '{"code":"print(2+2)"}'}}]}},
            {'finish_reason': 'stop', 'message': {'role': 'assistant', 'content': 'Four.'}},
        ]
        with patch('candidate_model_eval.tool_request', side_effect=replies) as requests, patch('candidate_model_eval.run_python', return_value={'ok': True, 'text': '4'}):
            self.assertEqual(python_pdf_screen('question', 7), ('Four.', 'stop', 1))
            self.assertEqual(requests.call_args.args[0][-1]['content'], '4')

    def test_parallel_calls_only_execute_first(self):
        def call(i):
            return {'id': str(i), 'type': 'function', 'function': {'name': 'python_calculate', 'arguments': '{"code":"print(2+2)"}'}}
        replies = [
            {'finish_reason': 'tool_calls', 'message': {'role': 'assistant', 'tool_calls': [call(1), call(2)]}},
            {'finish_reason': 'stop', 'message': {'role': 'assistant', 'content': 'Four.'}},
        ]
        with patch('candidate_model_eval.tool_request', side_effect=replies) as requests, patch('candidate_model_eval.run_python', return_value={'ok': True, 'text': '4'}) as run:
            self.assertEqual(python_pdf_screen('question', 7), ('Four.', 'stop', 1))
            self.assertEqual(run.call_count, 1)
            self.assertIn('not executed', requests.call_args.args[0][-1]['content'])

    def test_english(self):
        self.assertTrue(english_screen('Personification and metaphor. Shakespeare, 1609.'))
        self.assertFalse(english_screen('Personification and metaphor. Shakespeare, 1610.'))
        self.assertFalse(english_screen('Personification and metaphor, Shakespeare, 1609 First Folio.'))
        self.assertFalse(english_screen('Personification and metaphor, Shakespeare, 1609. Implied simile.'))


if __name__ == '__main__':
    unittest.main()
