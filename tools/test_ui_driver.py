"""Regressions for OCR data encountered by the native UI checks."""
import subprocess
import unittest
from unittest.mock import patch

import ui_driver


class OcrCoordinatesTest(unittest.TestCase):
    def test_literal_quote_does_not_consume_following_buttons(self):
        tsv = (
            "left\ttop\twidth\theight\ttext\n"
            '529\t512\t11\t9\t"\n'
            "491\t628\t26\t9\tSave\n"
            "373\t627\t38\t10\tCancel\n"
        )
        result = subprocess.CompletedProcess([], 0, stdout=tsv, stderr="")
        with patch.object(ui_driver, "screenshot"), patch.object(ui_driver.subprocess, "run", return_value=result):
            self.assertEqual(ui_driver.locate(None, "unused.png", "Save", minimum_y=600), (504, 632))
            self.assertEqual(ui_driver.locate(None, "unused.png", "Cancel", minimum_y=600), (392, 632))


if __name__ == "__main__":
    unittest.main()
