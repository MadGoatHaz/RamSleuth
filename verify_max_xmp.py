import unittest
from unittest.mock import patch, MagicMock
from ramsleuth_pkg.scanner import get_current_memory_settings

class TestMaxXmpVerification(unittest.TestCase):

    @patch('shutil.which')
    @patch('subprocess.run')
    def test_rated_xmp_formatting(self, mock_run, mock_which):
        # Mock dmidecode availability
        mock_which.side_effect = lambda x: '/usr/sbin/dmidecode' if x == 'dmidecode' else None

        # Minimal dmidecode output (needed for scanner to run without error)
        mock_dmidecode_output = """
        Handle 0x004A, DMI type 17, 92 bytes
        Memory Device
            Size: 16 GB
            Speed: 3600 MT/s
            Part Number: TEST_PART
            Configured Memory Speed: 3600 MT/s
            Manufacturer: TestMfg
        """
        
        mock_run.return_value = MagicMock(
            returncode=0,
            stdout=mock_dmidecode_output
        )

        # Scenario 1: Multi-line space separated (The likely failure case)
        # This mimics "XMP Profile 1: 3600 MT/s" on one line, and "18 22 22 42" on the next.
        spd_output_multiline = """
        ---
        Decoding EEPROM: /sys/bus/i2c/drivers/ee1004/0-0050
        
        XMP Profile 1: 3600 MT/s 1.35 V
             18 22 22 42
        """

        print("\n--- Testing Scenario 1: Multi-line space separated timings ---")
        settings = get_current_memory_settings(spd_output=spd_output_multiline, dimms_data=[])
        print(f"Scenario 1 Result: '{settings.get('Rated XMP')}'")
        
        # We expect it to capture all 4 timings
        self.assertEqual(settings.get('Rated XMP'), "3600 MT/s (CL18-22-22-42)")

        # Scenario 2: Mixed separators or just space separated on same line (Edge case)
        spd_output_mixed = """
        ---
        XMP Profile 2: 3200 MT/s 16 18 18 38 1.35 V
        """
        print("\n--- Testing Scenario 2: Same line space separated timings ---")
        settings_2 = get_current_memory_settings(spd_output=spd_output_mixed, dimms_data=[])
        print(f"Scenario 2 Result: '{settings_2.get('Rated XMP')}'")
        self.assertEqual(settings_2.get('Rated XMP'), "3200 MT/s (CL16-18-18-38)")

if __name__ == '__main__':
    unittest.main(argv=['first-arg-is-ignored'], exit=False)