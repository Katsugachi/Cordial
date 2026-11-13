#!/usr/bin/env python3
"""Exploratory, tool-free Qwen vision screening, not a Cordial release eval.

Runs only a local, pinned llama-server using its GGUF chat template. No model
output is executed. Correct numbers do not prove correct reasoning. External
wall-clock limits prevent a stalled server or response from occupying CI forever.
"""
import base64
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request

BASE = 'http://127.0.0.1:18964'


def numbers(text):
    return [float(m.group().replace(',', '')) for m in re.finditer(r'(?<![\d.])\d+(?:,\d{3})*(?:\.\d+)?(?![\d.])', text)]


def has_near(text, target, tolerance):
    return any(abs(value - target) <= tolerance for value in numbers(text))


def pdf_screen(answer):
    lower = answer.lower()
    # This numerical screening is deliberately conservative and not semantic verification.
    return (has_near(answer, 75.398, 0.1) and has_near(answer, 47.17, 0.1)
            and has_near(answer, 98.534, 0.1) and 'fit' in lower
            and not re.search(r'(?:not|doesn.t|cannot|can.t)\s+fit', lower))


def architecture_screen(answer):
    return all(has_near(answer, x, 0.01) for x in (336, 74, 148, 7104))


def english_screen(answer):
    lower = answer.lower()
    return all(term in lower for term in ('personification', 'metaphor', 'shakespeare', '1609'))


def request(messages, seed):
    data = json.dumps({'messages': messages, 'temperature': 0.1, 'seed': seed,
                       'max_tokens': 2048, 'stream': False}).encode()
    req = urllib.request.Request(BASE + '/v1/chat/completions', data,
                                 {'Content-Type': 'application/json'})
    with urllib.request.urlopen(req, timeout=1800) as response:
        body = json.load(response)
    choice = body['choices'][0]
    if choice.get('finish_reason') != 'stop':
        raise RuntimeError(f"generation did not finish normally: {choice.get('finish_reason')}")
    content = choice['message'].get('content')
    if not isinstance(content, str) or not content.strip():
        raise RuntimeError('model returned no text')
    return content


def main():
    server = os.environ['CANDIDATE_SERVER']
    weights = os.environ['CANDIDATE_WEIGHTS']
    projector = os.environ['CANDIDATE_PROJECTOR']
    image = Path(os.environ['CORDIAL_PDF_PAGE_IMAGE']).read_bytes()
    text = Path(os.environ['CORDIAL_PDF_PAGE_TEXT']).read_text()
    seed = int(os.environ.get('CORDIAL_LIVE_SEED', '0')) % (2**32 - 1)
    log_path = Path(os.environ['CANDIDATE_SERVER_LOG'])
    with log_path.open('w') as log:
        process = subprocess.Popen([server, '-m', weights, '--mmproj', projector,
                                    '--host', '127.0.0.1', '--port', '18964',
                                    '-c', '8192', '-np', '1', '-t', '3', '-ngl', '0', '--jinja'],
                                   stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError(f'llama-server exited: {process.returncode}')
                try:
                    with urllib.request.urlopen(BASE + '/health', timeout=2) as response:
                        if response.status == 200:
                            break
                except (OSError, urllib.error.URLError):
                    time.sleep(2)
            else:
                raise TimeoutError('llama-server did not become ready')
            print(f'EVAL candidate Qwen3-VL-4B Q4_K_M, seed={seed}, context=8192, tools=none', flush=True)
            questions = [
                ('pdf', [{'type': 'text', 'text': 'Complete each question in the worksheet image and explain your reasoning. Python and web tools are not available in this screening; do not claim to have used them. Extracted text follows; diagrams may contain additional labels:\n' + text},
                         {'type': 'image_url', 'image_url': {'url': 'data:image/png;base64,' + base64.b64encode(image).decode()}}], pdf_screen),
                ('architecture', 'A rectangular gallery is 21 m by 16 m. Calculate floor area and perimeter. Flooring packs cover 2.5 m², cost $48 each, and 10% extra area is needed for waste. Give the smallest whole number of packs and total cost, explaining your work.', architecture_screen),
                ('english', 'Explain the figures of speech in “The wind whispered secrets to the street” and “The library was a lighthouse in the storm”. Who wrote Sonnet 18 and when was it first published? Do not claim to have researched online.', english_screen),
            ]
            passed = True
            for label, content, check in questions:
                answer = request([{'role': 'user', 'content': content}], seed)
                ok = check(answer)
                passed &= ok
                print(f'EVAL candidate {label} screen={ok} answer={answer!r}', flush=True)
                print(f'::notice title=Candidate {label}::{answer[:2500].replace("%", "%25").replace(chr(10), "%0A").replace(chr(13), "%0D")}', flush=True)
            if not passed:
                raise SystemExit('Candidate screening failed; no production or release claim.')
        finally:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()


if __name__ == '__main__':
    try:
        main()
    except (KeyError, OSError, ValueError, TimeoutError, RuntimeError, json.JSONDecodeError) as error:
        print(f'Candidate screening error: {error}', file=sys.stderr, flush=True)
        sys.exit(1)
