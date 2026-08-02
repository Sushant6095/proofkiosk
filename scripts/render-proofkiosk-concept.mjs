import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import {
  accessSync,
  constants,
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const SCENE_COUNT = 7;
const FPS = 30;
const TRANSITION_SECONDS = 0.9;
const NARRATION_LEAD_SECONDS = 1.05;
const NARRATION_TAIL_SECONDS = 0.55;

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = dirname(scriptDir);
const sourceHtml = join(repoRoot, 'artifacts/video/proofkiosk-concept.html');
const narrationText = join(repoRoot, 'artifacts/video/narration.txt');
const workingDir = join(repoRoot, 'artifacts/video/rendered');
const framesDir = join(workingDir, 'frames');
const outputVideo = join(repoRoot, 'artifacts/ProofKiosk-Concept-Video.mp4');
const temporaryVideo = join(repoRoot, 'artifacts/.ProofKiosk-Concept-Video.rendering.mp4');

function findExecutable(environmentName, commandNames, absoluteCandidates = []) {
  const configured = process.env[environmentName];
  const candidates = [configured, ...absoluteCandidates].filter(Boolean);

  for (const candidate of candidates) {
    try {
      accessSync(candidate, constants.X_OK);
      return candidate;
    } catch {
      // Try the next explicit path before consulting PATH.
    }
  }

  for (const command of commandNames) {
    try {
      const resolved = execFileSync('/usr/bin/env', ['which', command], {
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'ignore'],
      }).trim();
      if (resolved) return resolved;
    } catch {
      // Try the next command name.
    }
  }

  throw new Error(`Unable to find ${commandNames[0]}. Set ${environmentName} to its executable path.`);
}

function probeDuration(ffprobePath, mediaPath) {
  const rawDuration = execFileSync(ffprobePath, [
    '-v', 'error',
    '-show_entries', 'format=duration',
    '-of', 'default=noprint_wrappers=1:nokey=1',
    mediaPath,
  ], { encoding: 'utf8' }).trim();
  const duration = Number(rawDuration);
  if (!Number.isFinite(duration) || duration <= 0) {
    throw new Error(`Invalid or silent narration asset: ${mediaPath}`);
  }
  return duration;
}

function validateVideo(ffprobePath, mediaPath, expectedDuration) {
  const probe = JSON.parse(execFileSync(ffprobePath, [
    '-v', 'error',
    '-show_streams',
    '-show_format',
    '-of', 'json',
    mediaPath,
  ], { encoding: 'utf8' }));

  const video = probe.streams?.find((stream) => stream.codec_type === 'video');
  const audio = probe.streams?.find((stream) => stream.codec_type === 'audio');
  const duration = Number(probe.format?.duration);
  if (!video || !audio || video.width !== 1920 || video.height !== 1080) {
    throw new Error('Rendered video must contain 1920x1080 video and an audio stream.');
  }
  if (!Number.isFinite(duration) || Math.abs(duration - expectedDuration) > 0.35) {
    throw new Error(`Rendered duration ${duration} did not match expected duration ${expectedDuration}.`);
  }
}

const ffmpegPath = findExecutable('FFMPEG_PATH', ['ffmpeg'], [
  '/opt/homebrew/bin/ffmpeg',
  '/usr/local/bin/ffmpeg',
  '/usr/bin/ffmpeg',
]);
const ffprobePath = findExecutable('FFPROBE_PATH', ['ffprobe'], [
  '/opt/homebrew/bin/ffprobe',
  '/usr/local/bin/ffprobe',
  '/usr/bin/ffprobe',
]);
const sayPath = findExecutable('SAY_PATH', ['say'], ['/usr/bin/say']);
const chromePath = findExecutable('CHROME_PATH', [
  'google-chrome',
  'chromium',
  'chromium-browser',
], [
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  '/Applications/Chromium.app/Contents/MacOS/Chromium',
]);

const require = createRequire(import.meta.url);
let chromium;
try {
  ({ chromium } = require(process.env.PLAYWRIGHT_PATH || 'playwright'));
} catch (error) {
  throw new Error('Playwright was not found. Install it locally or set PLAYWRIGHT_PATH.', { cause: error });
}

const narrationSegments = readFileSync(narrationText, 'utf8')
  .trim()
  .split(/\n\s*---\s*\n/)
  .map((segment) => segment.trim())
  .filter(Boolean);
if (narrationSegments.length !== SCENE_COUNT) {
  throw new Error(`Expected ${SCENE_COUNT} narration segments separated by ---, found ${narrationSegments.length}.`);
}

mkdirSync(framesDir, { recursive: true });

const pageErrors = [];
let browser;
try {
  browser = await chromium.launch({ headless: true, executablePath: chromePath });
  const page = await browser.newPage({ viewport: { width: 1920, height: 1080 }, deviceScaleFactor: 1 });
  page.on('pageerror', (error) => pageErrors.push(error.message));

  for (let scene = 1; scene <= SCENE_COUNT; scene += 1) {
    const url = new URL(pathToFileURL(sourceHtml));
    url.searchParams.set('scene', String(scene));
    await page.goto(url.toString(), { waitUntil: 'load' });

    const activeScenes = page.locator('.scene.active');
    const activeCount = await activeScenes.count();
    const activeNumber = activeCount === 1 ? await activeScenes.getAttribute('data-scene') : null;
    const activeBox = activeCount === 1 ? await activeScenes.boundingBox() : null;
    if (activeCount !== 1 || activeNumber !== String(scene) || !activeBox?.width || !activeBox?.height) {
      throw new Error(`Scene ${scene} did not render as the single visible scene.`);
    }

    await page.screenshot({
      path: join(framesDir, `scene-${String(scene).padStart(2, '0')}.png`),
      type: 'png',
    });
  }
} finally {
  await browser?.close();
}
if (pageErrors.length) {
  throw new Error(`Storyboard page errors: ${pageErrors.join('; ')}`);
}

const audioPaths = [];
const audioDurations = [];
for (let index = 0; index < narrationSegments.length; index += 1) {
  const sequence = String(index + 1).padStart(2, '0');
  const textPath = join(workingDir, `narration-${sequence}.txt`);
  const audioPath = join(workingDir, `narration-${sequence}.aiff`);
  writeFileSync(textPath, `${narrationSegments[index]}\n`, 'utf8');
  execFileSync(sayPath, ['-r', '178', '-o', audioPath, '-f', textPath], { stdio: 'inherit' });
  audioPaths.push(audioPath);
  audioDurations.push(probeDuration(ffprobePath, audioPath));
}

const sceneDurations = audioDurations.map((duration) => (
  Math.ceil((duration + NARRATION_LEAD_SECONDS + NARRATION_TAIL_SECONDS + TRANSITION_SECONDS) * 10) / 10
));
const sceneStarts = [0];
for (let index = 1; index < SCENE_COUNT; index += 1) {
  sceneStarts.push(sceneStarts[index - 1] + sceneDurations[index - 1] - TRANSITION_SECONDS);
}
const totalDuration = sceneStarts.at(-1) + sceneDurations.at(-1);

const ffmpegInputs = [];
for (let scene = 1; scene <= SCENE_COUNT; scene += 1) {
  ffmpegInputs.push(
    '-framerate', String(FPS),
    '-loop', '1',
    '-t', sceneDurations[scene - 1].toFixed(3),
    '-i', join(framesDir, `scene-${String(scene).padStart(2, '0')}.png`),
  );
}
for (const audioPath of audioPaths) ffmpegInputs.push('-i', audioPath);

const frameFilters = sceneDurations.map((duration, index) => {
  const frameCount = Math.max(1, Math.ceil(duration * FPS));
  const zoomIncrement = (0.025 / frameCount).toFixed(8);
  return `[${index}:v]zoompan=`
    + `z='min(zoom+${zoomIncrement},1.025)':`
    + `x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)':`
    + `d=1:s=1920x1080:fps=${FPS},trim=duration=${duration.toFixed(3)},setpts=PTS-STARTPTS[v${index}]`;
});

const xfadeFilters = [];
let previousVideo = 'v0';
for (let index = 1; index < SCENE_COUNT; index += 1) {
  const outputLabel = index === SCENE_COUNT - 1 ? 'vout' : `x${index}`;
  xfadeFilters.push(
    `[${previousVideo}][v${index}]xfade=transition=fade:duration=${TRANSITION_SECONDS}:`
    + `offset=${sceneStarts[index].toFixed(3)}[${outputLabel}]`,
  );
  previousVideo = outputLabel;
}

const audioFilters = audioDurations.map((duration, index) => {
  const delayMilliseconds = Math.round((sceneStarts[index] + NARRATION_LEAD_SECONDS) * 1000);
  return `[${SCENE_COUNT + index}:a]atrim=duration=${duration.toFixed(3)},asetpts=PTS-STARTPTS,`
    + `adelay=${delayMilliseconds}:all=1,volume=1.12[a${index}]`;
});
audioFilters.push(
  `${audioDurations.map((_, index) => `[a${index}]`).join('')}`
  + `amix=inputs=${SCENE_COUNT}:duration=longest:dropout_transition=0,`
  + `apad=whole_dur=${totalDuration.toFixed(3)}[aout]`,
);

rmSync(temporaryVideo, { force: true });
try {
  execFileSync(ffmpegPath, [
    '-y',
    '-hide_banner',
    '-loglevel', 'warning',
    ...ffmpegInputs,
    '-filter_complex', [...frameFilters, ...xfadeFilters, ...audioFilters].join(';'),
    '-map', '[vout]',
    '-map', '[aout]',
    '-t', totalDuration.toFixed(3),
    '-c:v', 'libx264',
    '-preset', 'medium',
    '-crf', '18',
    '-pix_fmt', 'yuv420p',
    '-c:a', 'aac',
    '-b:a', '192k',
    '-movflags', '+faststart',
    temporaryVideo,
  ], { stdio: 'inherit' });
  validateVideo(ffprobePath, temporaryVideo, totalDuration);
  renameSync(temporaryVideo, outputVideo);
} catch (error) {
  rmSync(temporaryVideo, { force: true });
  throw error;
}

console.log(JSON.stringify({ outputVideo, framesDir, sceneDurations, sceneStarts, totalDuration }));
