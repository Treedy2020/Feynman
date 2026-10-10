#!/usr/bin/env python3
"""Run the real bridge with a tiny offline Slate package, never bypass its logic."""
import os
from pathlib import Path
import sys
os.environ['PYTHONPATH'] = str(Path(__file__).parent / 'native-package')
os.execv(sys.executable, [sys.executable, *sys.argv[1:]])
