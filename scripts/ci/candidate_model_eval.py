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
    return (has_near(answer, 75.398, 0.01) and has_near(answer, 47.17, 0.02)
            and has_near(answer, 98.534, 0.0005) and 'fit' in lower
            and not re.search(r'(?:not|doesn.t|cannot|can.t)\s+fit', lower))


def architecture_screen(answer):
    return all(has_near(answer, x, 0.01) for x in (336, 74, 148, 7104))


def english_screen(answer):
    lower = answer.lower()
    return (all(term in lower for term in ('personification', 'metaphor', 'shakespeare', '1609'))
            and 'first folio' not in lower and 'simile' not in lower)


def request(messages, seed):
    data = json.dumps({'messages': messages, 'temperature': 0.1, 'seed': seed,
                       'max_tokens': 2048, 'stream': False}).encode()
    req = urllib.request.Request(BASE + '/v1/chat/completions', data,
                                 {'Content-Type': 'application/json'})
    with urllib.request.urlopen(req, timeout=1800) as response:
        body = json.load(response)
    choice = body['choices'][0]
    content = choice['message'].get('content')
    if not isinstance(content, str) or not content.strip():
        raise RuntimeError('model returned no text')
    return content, choice.get('finish_reason')


PYTHON_TOOL = [{"type": "function", "function": {
    "name": "python_calculate", "description": "Run a short Python calculation in an isolated in-memory interpreter. No files, network, subprocesses or persistent variables; print results. Scripts are not saved to disk.",
    "parameters": {"type": "object", "properties": {"code": {"type": "string"}}, "required": ["code"]}}}]


def tool_request(messages, seed):
    data = json.dumps({'messages': messages, 'tools': PYTHON_TOOL, 'tool_choice': 'auto',
                       'temperature': 0.1, 'seed': seed, 'max_tokens': 2048, 'stream': False}).encode()
    req = urllib.request.Request(BASE + '/v1/chat/completions', data, {'Content-Type': 'application/json'})
    with urllib.request.urlopen(req, timeout=1800) as response:
        return json.load(response)['choices'][0]


def run_python(code):
    if not isinstance(code, str) or not code or len(code.encode()) > 8192:
        return {'ok': False, 'text': 'Invalid code: expected 1-8192 bytes.'}
    process = subprocess.run([os.environ['CANDIDATE_PYTHON_BRIDGE']], input=code, text=True,
                             capture_output=True, timeout=90, check=True)
    return json.loads(process.stdout)


def python_pdf_screen(content, seed):
    # Deliberately bounded experiment, not the Cordial agent or its checklist.
    messages = [{'role': 'system', 'content': 'Use python_calculate for numerical calculations. Only report outputs you obtained. No script files are needed.'},
                {'role': 'user', 'content': content}]
    successful = 0
    for _ in range(6):
        choice = tool_request(messages, seed)
        message = choice['message']
        calls = message.get('tool_calls') or []
        if not calls:
            return message.get('content') or '', choice.get('finish_reason'), successful
        # Mirror Cordial's one-call-at-a-time policy: only the first call is
        # executed; every other call receives an explicit non-execution result.
        messages.append(message)
        for index, call in enumerate(calls):
            call_id = call.get('id')
            if not isinstance(call_id, str) or not call_id:
                raise RuntimeError('candidate tool call has no id')
            if index > 0:
                result = {'ok': False, 'text': 'Only one tool call may run per response. This call was not executed; review the first result.'}
            else:
                function = call.get('function', {})
                arguments = function.get('arguments', '{}')
                arguments = json.loads(arguments) if isinstance(arguments, str) else arguments
                if function.get('name') != 'python_calculate' or not isinstance(arguments, dict):
                    raise RuntimeError('candidate requested an unsupported tool')
                code = arguments.get('code')
                result = run_python(code)
                successful += int(result.get('ok') is True)
                diagnostic = json.dumps({'code': code, 'result': result})
                print('EVAL candidate Python ' + diagnostic, flush=True)
                safe = diagnostic[:2500].replace('%', '%25').replace('\n', '%0A').replace('\r', '%0D')
                print(f'::notice title=Candidate Python call::{safe}', flush=True)
            messages.append({'role': 'tool', 'tool_call_id': call_id, 'content': result.get('text', '')})
    raise RuntimeError('candidate exceeded six tool-response rounds')


def main():
    server = os.environ['CANDIDATE_SERVER']
    weights = os.environ['CANDIDATE_WEIGHTS']
    projector = os.environ['CANDIDATE_PROJECTOR']
    image = Path(os.environ['CORDIAL_PDF_PAGE_IMAGE']).read_bytes()
    text = Path(os.environ['CORDIAL_PDF_PAGE_TEXT']).read_text()
    seed = int(os.environ.get('CORDIAL_LIVE_SEED', '0')) % (2**32 - 1)
    log_path = Path(os.environ['CANDIDATE_SERVER_LOG'])
    report = Path(os.environ['CANDIDATE_REPORT_PATH'])
    report.write_text('')

    def record(label, answer, ok):
        with report.open('a') as output:
            output.write(json.dumps({'label': label, 'screen': ok, 'answer': answer}) + '\n')
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
                ('pdf', [{'type': 'text', 'text': 'Complete each question in the worksheet image and explain your reasoning. Extracted text follows; diagrams may contain additional labels:\n' + text},
                         {'type': 'image_url', 'image_url': {'url': 'data:image/png;base64,' + base64.b64encode(image).decode()}}], pdf_screen),
                ('architecture', 'A rectangular gallery is 21 m by 16 m. Calculate floor area and perimeter. Flooring packs cover 2.5 m², cost $48 each, and 10% extra area is needed for waste. Give the smallest whole number of packs and total cost, explaining your work.', architecture_screen),
                ('english', 'Explain the figures of speech in “The wind whispered secrets to the street” and “The library was a lighthouse in the storm”. Who wrote Sonnet 18 and when was it first published? Do not claim to have researched online.', english_screen),
            ]
            passed = True
            for label, content, check in questions:
                answer, finish = request([{'role': 'system', 'content': 'Python and web tools are unavailable; do not claim to have used them.'}, {'role': 'user', 'content': content}], seed)
                ok = finish == 'stop' and check(answer)
                print(f'EVAL candidate {label} finish={finish}', flush=True)
                passed &= ok
                record(label, answer, ok)
                print(f'EVAL candidate {label} screen={ok} answer={answer!r}', flush=True)
                print(f'::notice title=Candidate {label}::{answer[:2500].replace("%", "%25").replace(chr(10), "%0A").replace(chr(13), "%0D")}', flush=True)
            # Regional images are a diagnostic only, not the desktop PDF path.
            # Evaluate independently even if the full-page screening failed.
            prefix = os.environ.get('CANDIDATE_REGION_PREFIX')
            if prefix:
                checks = [lambda a: has_near(a, 75.398, 0.01),
                          lambda a: has_near(a, 47.17, 0.02) and 'fit' in a.lower() and not re.search(r'(?:not|doesn.t|cannot|can.t)\s+fit', a.lower()),
                          lambda a: has_near(a, 98.534, 0.0005)]
                for index, check in enumerate(checks, 1):
                    crop = Path(f'{prefix}-{index}.png').read_bytes()
                    content = [{'type': 'text', 'text': f'Answer only question {index} shown in this region of the worksheet. Explain your reasoning concisely. Python and web tools are unavailable; do not claim to have used them.'},
                               {'type': 'image_url', 'image_url': {'url': 'data:image/png;base64,' + base64.b64encode(crop).decode()}}]
                    answer, finish = request([{'role': 'user', 'content': content}], seed)
                    ok = finish == 'stop' and check(answer)
                    print(f'EVAL candidate region-{index} finish={finish}', flush=True)
                    record(f'region-{index}', answer, ok)
                    print(f'EVAL candidate region-{index} screen={ok} answer={answer!r}', flush=True)
                    print(f'::notice title=Candidate region {index}::{answer[:2500].replace("%", "%25").replace(chr(10), "%0A").replace(chr(13), "%0D")}', flush=True)
            # Separate bounded tool-use diagnostic. Only the real isolated WASI
            # calculator runs model code; no network or host shell is exposed.
            if os.environ.get('CANDIDATE_PYTHON_BRIDGE'):
                pdf_content = questions[0][1]
                answer, finish, successful = python_pdf_screen(pdf_content, seed)
                ok = finish == 'stop' and successful > 0 and pdf_screen(answer)
                passed &= ok
                record('pdf-python', answer, ok)
                print(f'EVAL candidate pdf-python successful_calls={successful} finish={finish}', flush=True)
                print(f'::notice title=Candidate PDF Python::{answer[:2500].replace("%", "%25").replace(chr(10), "%0A").replace(chr(13), "%0D")}', flush=True)
                if not ok:
                    print('::notice title=Candidate Python screen::Did not pass the bounded PDF/Python screen.', flush=True)
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
    except (KeyError, OSError, ValueError, TypeError, IndexError, TimeoutError, RuntimeError, subprocess.SubprocessError) as error:
        message = (type(error).__name__ + ': ' + str(error)).replace('%', '%25').replace('\n', '%0A').replace('\r', '%0D')
        print(f'::error title=Candidate screening error::{message}', flush=True)
        print(f'Candidate screening error: {error}', file=sys.stderr, flush=True)
        sys.exit(1)
