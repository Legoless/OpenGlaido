// Decorative motion only: this page never opens the microphone or calls a model.
const toggle = document.querySelector('#motion-toggle');
const label = document.querySelector('#motion-label');
const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)');

function setMotion(running) {
  document.body.dataset.motion = running ? 'running' : 'paused';
  label.textContent = running ? 'Pause motion' : 'Play motion';
  toggle.setAttribute('aria-label', running ? 'Pause animations' : 'Play animations');
}

setMotion(!reducedMotion.matches);
toggle.hidden = false;
toggle.addEventListener('click', () => {
  setMotion(document.body.dataset.motion !== 'running');
});
reducedMotion.addEventListener('change', (event) => setMotion(!event.matches));
