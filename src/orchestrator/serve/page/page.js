// Pumice test page (p15-serve-transcriptions/D8): served by `pumice serve`
// at `/`, it only talks to its own origin. Everything is rendered as text,
// and audio and results stay in this page's memory.
'use strict';

const $ = id => document.getElementById(id);
const REQUEST_TIMEOUT_MS = 200000;
const MODELS_TIMEOUT_MS = 15000;
const MAX_FILE_BYTES = 25000000;
const MAX_SECONDS = 120;
const WEBM_MIME = 'audio/webm;codecs=opus';

let models = [];
let loadingModels = false;
let starting = false;
let recording = null;
// The running request: {controller, timedOut}.
let pending = null;
let playbackUrl = null;

function controls() {
  const working = !!pending || starting || loadingModels;
  for (const id of ['model', 'refresh', 'language', 'recFormat', 'file', 'upload']) {
    $(id).disabled = working || !!recording;
  }
  $('record').disabled = working || !navigator.mediaDevices?.getUserMedia;
  $('record').textContent = recording ? 'Stop & transcribe' : starting ? 'Opening microphone…' : 'Record';
  $('record').classList.toggle('recording', !!recording);
  $('cancel').disabled = !pending && !recording;
}

function showError(message) {
  $('error').textContent = message;
  $('error').hidden = !message;
}

async function readBody(response) {
  const raw = await response.text();
  try {
    return [raw, JSON.parse(raw)];
  } catch {
    return [raw, null];
  }
}

const HINTS = {
  rate_limit_exceeded: 'Busy: another dictation or transcription is running. Try again in a moment.',
  model_not_found: 'This model is not published on this server. Refresh the model list.',
};

// An OpenAI error object, readably.
function errorText(status, body) {
  const error = body?.error;
  if (!error || typeof error !== 'object') return `HTTP ${status}: the request failed.`;
  const hint = HINTS[error.code]
    ?? (status === 503 ? 'The service is not ready (starting, restarting, stopped or shutting down).' : '');
  return `${hint ? `${hint}\n` : ''}${error.message ?? 'No message.'}\n`
    + `HTTP ${status} · type ${error.type ?? '—'} · code ${error.code ?? '—'} · param ${error.param ?? '—'}`;
}

// Models

async function loadModels() {
  if (loadingModels || pending || recording || starting) return;
  loadingModels = true;
  controls();
  $('modelInfo').textContent = 'Loading models…';
  try {
    const response = await fetch('/v1/models', {cache: 'no-store', signal: AbortSignal.timeout(MODELS_TIMEOUT_MS)});
    const [, body] = await readBody(response);
    if (!response.ok) throw new Error(errorText(response.status, body));
    if (!Array.isArray(body?.data)) throw new Error('The server did not answer an OpenAI model list.');
    const previous = $('model').value;
    models = body.data.filter(model => typeof model?.id === 'string');
    $('model').replaceChildren(...(models.length
      ? models.map(model => new Option(model.id, model.id))
      : [new Option('No published models', '')]));
    if (models.some(model => model.id === previous)) $('model').value = previous;
    showModel();
  } catch (error) {
    models = [];
    $('model').replaceChildren(new Option('Models unavailable', ''));
    $('modelInfo').textContent = `Could not load the models: ${error.message}`;
  } finally {
    loadingModels = false;
    controls();
  }
}

function showModel() {
  const model = models.find(model => model.id === $('model').value);
  if (!model) {
    $('modelInfo').textContent = models.length ? ''
      : 'No transcription models are available. Check orchestrator.yml and restart pumice serve.';
    return;
  }
  const details = model.pumice ?? {};
  const post = details.post_processing ?? {};
  const ready = details.ready === true ? 'ready' : `not ready (${details.unavailable ?? 'unknown'})`;
  $('modelInfo').textContent = `Preset: ${post.preset ?? 'off'} · AI route: ${post.relay_route ?? '—'}`
    + ` · audio model: ${details.transcription_model ?? '—'} · ${ready}`;
}

// Transcription

function describeFile(file) {
  return `${file.name || '(unnamed)'} · ${file.type || 'unknown type'} · ${file.size.toLocaleString()} bytes`;
}

function showPlayback(file) {
  if (playbackUrl) URL.revokeObjectURL(playbackUrl);
  playbackUrl = URL.createObjectURL(file);
  $('playback').src = playbackUrl;
  $('playback').hidden = false;
}

async function transcribe(file) {
  if (pending || recording || starting) return;
  const model = $('model').value;
  if (!model) return showError('Choose a model first (refresh the list if it is empty).');
  if (!file || file.size === 0 || file.size > MAX_FILE_BYTES) {
    return showError('Choose non-empty audio up to 25,000,000 bytes.');
  }
  const form = new FormData();
  form.append('file', file, file.name || 'audio');
  form.append('model', model);
  const language = $('language').value.trim();
  if (language) form.append('language', language);
  form.append('response_format', 'json');

  const request = {controller: new AbortController(), timedOut: false};
  pending = request;
  controls();
  showError('');
  $('result').hidden = true;
  for (const id of ['status', 'elapsed', 'raw']) $(id).textContent = '';
  $('sent').textContent = describeFile(file);
  $('request').hidden = false;
  showPlayback(file);
  $('progress').textContent = 'Transcribing… (Cancel stops it on the server too)';
  const timer = setTimeout(() => {
    request.timedOut = true;
    request.controller.abort();
  }, REQUEST_TIMEOUT_MS);
  const start = performance.now();
  let progress = 'Request failed.';
  try {
    const response = await fetch('/v1/audio/transcriptions', {
      method: 'POST', body: form, cache: 'no-store', signal: request.controller.signal,
    });
    const [raw, body] = await readBody(response);
    $('status').textContent = `${response.status} ${response.statusText}`.trim();
    $('raw').textContent = raw;
    if (!response.ok) showError(errorText(response.status, body));
    else if (typeof body?.text !== 'string') showError('Expected a JSON answer with a text string.');
    else {
      showResult(body);
      progress = body.text === '' ? 'Done: empty text (silence).' : 'Done.';
    }
  } catch (error) {
    if (request.timedOut) {
      showError(`No answer within ${REQUEST_TIMEOUT_MS / 1000} s, so the page gave up. `
        + 'Closing the request also cancels the transcription on the server.');
    } else if (error.name === 'AbortError') {
      progress = 'Cancelled.';
    } else {
      showError(`The request failed: ${error.message}\nIs pumice serve still running?`);
    }
  } finally {
    clearTimeout(timer);
    $('elapsed').textContent = String(Math.round(performance.now() - start));
    $('progress').textContent = progress;
    pending = null;
    controls();
  }
}

const STATUS_TEXT = {
  off: 'No post-processing: the final text is the raw transcript.',
  applied: 'The formatter and the AI route ran.',
  skipped: 'Silent transcript: no AI call.',
  failed: 'Post-processing failed: the final text is the raw transcript.',
};

function row(...cells) {
  const tr = document.createElement('tr');
  for (const cell of cells) {
    const td = document.createElement('td');
    td.textContent = cell ?? '—';
    tr.append(td);
  }
  return tr;
}

function showResult(body) {
  const details = body.pumice ?? {};
  const post = details.post_processing ?? {};
  $('text').textContent = body.text;
  $('copied').textContent = '';
  $('ppStatus').textContent = post.status ?? '—';
  $('ppStatus').classList.toggle('failed', post.status === 'failed');
  $('ppDetail').textContent = `${STATUS_TEXT[post.status] ?? ''} Preset: ${post.preset ?? 'off'} · AI route: ${post.relay_route ?? '—'}`;
  const failure = post.error;
  $('ppError').hidden = !failure;
  $('ppError').textContent = failure
    ? `${failure.message ?? 'No message.'}\nstage ${failure.stage ?? '—'} · code ${failure.code ?? '—'}`
      + (failure.cause_code ? ` · cause ${failure.cause_code}` : '')
    : '';
  const transcript = details.transcript;
  $('transcript').textContent = transcript?.text ?? '';
  $('transcriptInfo').textContent = transcript
    ? `Language ${transcript.language ?? '—'} · ${transcript.audio_seconds ?? '—'} s of audio`
    : 'The answer has no raw transcript.';
  const rows = [];
  for (const [stage, report] of Object.entries(details.stages ?? {})) {
    rows.push(row(stage, report?.status, report?.elapsed_ms == null ? '—' : `${report.elapsed_ms} ms`));
    if (report?.model_load_ms != null) rows.push(row('  model load', '', `${report.model_load_ms} ms`));
    if (report?.inference_ms != null) rows.push(row('  inference', '', `${report.inference_ms} ms`));
  }
  rows.push(row('total on the server', '', details.timings_ms?.total == null ? '—' : `${details.timings_ms.total} ms`));
  $('timings').replaceChildren(...rows);
  $('result').hidden = false;
}

async function copyText() {
  const text = $('text').textContent;
  try {
    await navigator.clipboard.writeText(text);
    $('copied').textContent = 'Copied.';
  } catch {
    // Without clipboard access, select the text for a manual copy.
    getSelection().selectAllChildren($('text'));
    $('copied').textContent = 'Selected: copy it with the keyboard.';
  }
}

// Recording: the browser's own WebM/Opus through MediaRecorder, or a WAV
// built in the page; both release the microphone when they stop.

const webmSupported = () => typeof MediaRecorder !== 'undefined' && MediaRecorder.isTypeSupported(WEBM_MIME);

function updateFormatNote() {
  $('formatNote').textContent = $('recFormat').value === 'webm' && !webmSupported()
    ? 'This browser cannot record WebM/Opus; recordings fall back to WAV.'
    : '';
}

// Mono 16-bit PCM WAV.
function encodeWav(samples, sampleRate) {
  const buffer = new ArrayBuffer(44 + samples.length * 2);
  const view = new DataView(buffer);
  const ascii = (offset, value) => {
    for (let i = 0; i < value.length; i++) view.setUint8(offset + i, value.charCodeAt(i));
  };
  ascii(0, 'RIFF'); view.setUint32(4, buffer.byteLength - 8, true); ascii(8, 'WAVE');
  ascii(12, 'fmt '); view.setUint32(16, 16, true); view.setUint16(20, 1, true); view.setUint16(22, 1, true);
  view.setUint32(24, sampleRate, true); view.setUint32(28, sampleRate * 2, true);
  view.setUint16(32, 2, true); view.setUint16(34, 16, true);
  ascii(36, 'data'); view.setUint32(40, samples.length * 2, true);
  for (let i = 0; i < samples.length; i++) {
    const s = Math.max(-1, Math.min(1, samples[i]));
    view.setInt16(44 + i * 2, Math.round(s * (s < 0 ? 32768 : 32767)), true);
  }
  return buffer;
}

async function release(r) {
  clearInterval(r.timer);
  r.processor.onaudioprocess = null;
  r.source.disconnect(); r.processor.disconnect(); r.mute.disconnect();
  r.stream.getTracks().forEach(track => track.stop());
  await r.context.close();
  $('level').value = 0;
}

async function stopRecording(send = true) {
  const r = recording;
  if (!r) return;
  recording = null;
  starting = true;
  controls();
  let file;
  if (r.mode === 'webm') {
    if (r.recorder.state !== 'inactive') {
      const stopped = new Promise(resolve => { r.recorder.onstop = resolve; });
      r.recorder.stop();
      await stopped;
    }
    file = new File([new Blob(r.chunks, {type: 'audio/webm'})], 'recording.webm', {type: 'audio/webm'});
  } else {
    const samples = new Float32Array(r.count);
    let offset = 0;
    for (const chunk of r.chunks) { samples.set(chunk, offset); offset += chunk.length; }
    file = new File([encodeWav(samples, r.rate)], 'recording.wav', {type: 'audio/wav'});
  }
  await release(r);
  starting = false;
  controls();
  if (send) await transcribe(file);
  else $('progress').textContent = 'Recording discarded.';
}

async function startRecording() {
  if (!$('model').value) return showError('Choose a model first (refresh the list if it is empty).');
  starting = true;
  controls();
  showError('');
  let stream, context;
  try {
    stream = await navigator.mediaDevices.getUserMedia({audio: {channelCount: 1}, video: false});
    context = new AudioContext();
    await context.resume();
    const source = context.createMediaStreamSource(stream);
    // ScriptProcessor feeds the level meter (and the WAV mode) without a
    // separate worklet file.
    const processor = context.createScriptProcessor(4096, 1, 1);
    const mute = context.createGain();
    mute.gain.value = 0;
    const wantWebm = $('recFormat').value === 'webm';
    const useWebm = wantWebm && webmSupported();
    if (wantWebm && !useWebm) $('formatNote').textContent = 'This browser cannot record WebM/Opus; this recording uses WAV.';
    const r = {
      stream, context, source, processor, mute, rate: context.sampleRate,
      mode: useWebm ? 'webm' : 'wav', chunks: [], count: 0, start: performance.now(),
    };
    recording = r;
    processor.onaudioprocess = event => {
      if (recording !== r) return;
      const input = event.inputBuffer.getChannelData(0);
      if (r.mode === 'wav') {
        const remaining = Math.max(0, Math.floor(r.rate * MAX_SECONDS) - r.count);
        const chunk = input.slice(0, remaining);
        r.chunks.push(chunk);
        r.count += chunk.length;
        if (r.count >= r.rate * MAX_SECONDS) void stopRecording();
      }
      let sum = 0;
      for (const value of input) sum += value * value;
      $('level').value = input.length ? Math.min(1, Math.sqrt(sum / input.length) * 4) : 0;
    };
    source.connect(processor); processor.connect(mute); mute.connect(context.destination);
    if (useWebm) {
      const recorder = new MediaRecorder(stream, {mimeType: WEBM_MIME});
      // No `recording === r` check: the final chunk arrives after
      // stopRecording() has cleared `recording`.
      recorder.ondataavailable = event => { if (event.data?.size) r.chunks.push(event.data); };
      recorder.onerror = event => {
        if (recording !== r) return;
        showError(`Recorder failed: ${event.error?.message || 'unknown error'}`);
        void stopRecording(false);
      };
      r.recorder = recorder;
      recorder.start(1000);
    }
    const tick = () => {
      // MediaRecorder output can run slightly past the wall clock; stop
      // WebM a second early so it stays under the server's 120 s limit.
      const limit = r.mode === 'webm' ? MAX_SECONDS - 1 : MAX_SECONDS;
      const seconds = Math.min(limit, Math.floor((performance.now() - r.start) / 1000));
      $('timer').textContent = `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')} / 2:00`;
      if (seconds >= limit) void stopRecording();
    };
    r.timer = setInterval(tick, 100);
    tick();
    $('progress').textContent = 'Recording… (Cancel discards it)';
  } catch (error) {
    stream?.getTracks().forEach(track => track.stop());
    if (context) await context.close();
    recording = null;
    showError(`Microphone unavailable: ${error.message}`);
  } finally {
    starting = false;
    controls();
  }
}

function cancel() {
  if (pending) pending.controller.abort();
  else if (recording) void stopRecording(false);
}

$('model').addEventListener('change', showModel);
$('refresh').addEventListener('click', loadModels);
$('recFormat').addEventListener('change', updateFormatNote);
$('record').addEventListener('click', () => recording ? stopRecording() : startRecording());
$('upload').addEventListener('click', () => transcribe($('file').files[0]));
$('cancel').addEventListener('click', cancel);
$('copy').addEventListener('click', copyText);
window.addEventListener('pagehide', () => {
  if (!recording) return;
  recording.recorder?.stop();
  recording.stream.getTracks().forEach(track => track.stop());
  void recording.context.close();
});
$('notice').textContent = navigator.mediaDevices?.getUserMedia
  ? 'The browser asks for microphone permission when you click Record.'
  : 'Microphone access needs a secure context: open this page as 127.0.0.1 on this machine, '
    + 'or through an SSH forward to the same port. File uploads still work.';
updateFormatNote();
controls();
void loadModels();
