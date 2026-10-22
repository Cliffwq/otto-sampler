// Apply the saved palette before the first paint, including live OS changes.
(() => {
  const media = matchMedia('(prefers-color-scheme: dark)');
  let mode = 'system';
  let palette = 'mpc60';
  try {
    const saved = localStorage.getItem('otto-sampler-theme');
    if (['light', 'dark', 'system'].includes(saved)) mode = saved;
    const savedPalette = localStorage.getItem('otto-sampler-palette');
    if (['mpc2000xl', 'mpc60', 'sp1200', 'sp404'].includes(savedPalette)) palette = savedPalette;
  } catch (err) { console.warn('Cannot read theme preference', err); }
  function apply() {
    const theme = mode === 'system' ? (media.matches ? 'dark' : 'light') : mode;
    document.documentElement.dataset.theme = theme;
    document.documentElement.dataset.palette = palette;
    document.documentElement.style.colorScheme = theme;
  }
  media.addEventListener('change', apply);
  apply();
  document.addEventListener('DOMContentLoaded', () => {
    const modal = document.getElementById('settings-modal');
    const open = document.getElementById('settings-btn');
    const close = document.getElementById('settings-close-btn');
    const paletteSelect = document.getElementById('palette-select');
    paletteSelect.value = palette;
    paletteSelect.addEventListener('change', () => {
      palette = paletteSelect.value;
      try { localStorage.setItem('otto-sampler-palette', palette); }
      catch (err) { console.warn('Cannot save palette preference', err); }
      apply();
    });
    const options = Array.from(modal.querySelectorAll('input[name="theme"]'));
    window.addEventListener('otto-reset-preferences', () => {
      mode = 'system'; palette = 'mpc60'; paletteSelect.value = palette;
      options.forEach(option => option.checked = option.value === mode);
      apply();
    });
    function dismiss() { modal.classList.add('hidden'); open.focus(); }
    open.addEventListener('click', () => {
      modal.classList.remove('hidden');
      paletteSelect.focus();
    });
    close.addEventListener('click', dismiss);
    modal.addEventListener('click', event => { if (event.target === modal) dismiss(); });
    modal.addEventListener('keydown', event => {
      if (event.key === 'Escape') { event.stopPropagation(); dismiss(); }
      if (event.key === 'Tab') {
        const controls = [...modal.querySelectorAll('button, select, input')].filter(el =>
          !el.disabled && el.getClientRects().length && (el.type !== 'radio' || el.checked));
        const first = controls[0], last = controls[controls.length - 1];
        if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
      }
    });
    options.forEach(option => {
      option.checked = option.value === mode;
      option.addEventListener('change', () => {
        if (!option.checked) return;
        mode = option.value;
        try { localStorage.setItem('otto-sampler-theme', mode); }
        catch (err) { console.warn('Cannot save theme preference', err); }
        apply();
      });
    });
  });
})();
