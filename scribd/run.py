"""Entry point used by ytdlp-web to run the bundled Scribd downloader.

`scribd-downloader.py` is copied unchanged from
https://github.com/themrsami/scribd-downloader (MIT, commit 9d09d00), so it
can be updated by copying a newer version over it. This wrapper only adds
what a server needs:

  CHROME_BIN     path to the Chrome/Chromium binary
  CHROMEDRIVER   path to a matching chromedriver

Both fall back to `chromium`/`chromedriver` on PATH. Naming them skips
Selenium Manager, which would otherwise try to download Chrome and
chromedriver at runtime.
"""

import importlib.util
import os
import shutil
import sys
import types

from selenium import webdriver
from selenium.webdriver.chrome.service import Service

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location(
    "scribd_downloader", os.path.join(HERE, "scribd-downloader.py")
)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)

chrome_bin = os.getenv("CHROME_BIN") or shutil.which("chromium")
chromedriver = os.getenv("CHROMEDRIVER") or shutil.which("chromedriver")

if chrome_bin:
    _build_options = mod.build_chrome_options

    def build_chrome_options(profile_dir):
        options = _build_options(profile_dir)
        options.binary_location = chrome_bin
        return options

    mod.build_chrome_options = build_chrome_options

if chromedriver:
    def chrome(options=None, **kwargs):
        kwargs.setdefault("service", Service(executable_path=chromedriver))
        return webdriver.Chrome(options=options, **kwargs)

    mod.webdriver = types.SimpleNamespace(Chrome=chrome)

if __name__ == "__main__":
    mod.main(sys.argv[1:])
