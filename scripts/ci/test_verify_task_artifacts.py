#!/usr/bin/env python3
import tempfile
import unittest
from pathlib import Path
from zipfile import ZipFile

from verify_task_artifacts import verify


class VerifierTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.addCleanup(self.tmp.cleanup)

    def docx(self, name, content):
        xml = ('<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
               '<w:body><w:p><w:r><w:t>' + content + '</w:t></w:r></w:p></w:body></w:document>')
        with ZipFile(self.root / name, 'w') as archive:
            archive.writestr('word/document.xml', xml)

    def test_accepts_correct_artifacts(self):
        (self.root / 'totals.txt').write_text('Item A: 34\nItem B: 87\nItem C: 172\nGrand total: 293\n')
        self.docx('done.docx', 'Question 1: Answer 46. Question 2: Answer 26.')
        self.assertEqual(verify(self.root, 17, 29, 43), [])

    def test_does_not_accept_answers_in_original_or_unassociated_numbers(self):
        (self.root / 'totals.txt').write_text('Item A: 999\nItem B: 999\nItem C: 999\nTotals: 34 87 172 293')
        self.docx('worksheet.docx', '46 and 26')
        self.assertEqual(len(verify(self.root, 17, 29, 43)), 5)

    def test_rejects_answers_in_wrong_question_sections(self):
        (self.root / 'totals.txt').write_text('Item A: 34\nItem B: 87\nItem C: 172\nGrand total: 293\n')
        self.docx('done.docx', 'Question 1: Answer 26. Question 2: Answer 46.')
        self.assertEqual(len(verify(self.root, 17, 29, 43)), 1)

    def test_rejects_substrings_and_invalid_docx(self):
        (self.root / 'totals.txt').write_text('Item A: 1340\nItem B: 870\nItem C: 1720\nGrand total: 2930')
        (self.root / 'broken.docx').write_text('not a zip')
        self.assertEqual(len(verify(self.root, 17, 29, 43)), 5)


if __name__ == '__main__':
    unittest.main()
