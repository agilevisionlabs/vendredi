const dialog = document.querySelector('dialog');
const viewer = dialog.querySelector('.viewer');
const image = viewer.querySelector('img');
const zoom = document.querySelector('#zoom');
const closeButton = document.querySelector('#close');

function setZoom(actual) {
  viewer.classList.toggle('zoom', actual);
  zoom.setAttribute('aria-pressed', String(actual));
  zoom.textContent = actual ? 'Fit to window' : 'Actual pixels';
  viewer.scrollTo(0, 0);
}

document.querySelectorAll('a[data-title]').forEach(link => {
  link.addEventListener('click', event => {
    if (!dialog.showModal || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    const caption = link.querySelector('img').alt;
    image.src = link.href;
    image.alt = caption;
    dialog.querySelector('h2').textContent = link.dataset.title;
    dialog.querySelector('.viewer-note').textContent = caption;
    setZoom(false);
    dialog.showModal();
    closeButton.focus();
  });
});
zoom.addEventListener('click', () => setZoom(!viewer.classList.contains('zoom')));
closeButton.addEventListener('click', () => dialog.close());
dialog.addEventListener('click', event => {
  if (event.target !== dialog) return;
  const bounds = dialog.getBoundingClientRect();
  if (event.clientX < bounds.left || event.clientX > bounds.right ||
      event.clientY < bounds.top || event.clientY > bounds.bottom) dialog.close();
});

const mobile = document.querySelector('.nav-mobile');
mobile.querySelectorAll('nav a').forEach(link => link.addEventListener('click', () => {
  mobile.open = false;
}));
const chapters = [...document.querySelectorAll('.chapter')];
const links = [...document.querySelectorAll('.contents a')];
let scheduled = false;
function updateChapter() {
  const current = chapters.filter(section => section.getBoundingClientRect().top <= 120).at(-1) || chapters[0];
  for (const link of links) {
    if (link.hash === '#' + current.id) link.setAttribute('aria-current', 'location');
    else link.removeAttribute('aria-current');
  }
  scheduled = false;
}
window.addEventListener('scroll', () => {
  if (!scheduled) {
    scheduled = true;
    requestAnimationFrame(updateChapter);
  }
}, {passive:true});
updateChapter();

document.querySelectorAll('.snippet').forEach(block => {
  const button = document.createElement('button');
  button.type = 'button';
  button.textContent = 'Copy';
  button.setAttribute('aria-label', 'Copy ' + block.querySelector('header span').textContent);
  block.querySelector('header').append(button);
  button.addEventListener('click', async () => {
    const code = block.querySelector('code');
    try {
      await navigator.clipboard.writeText(code.textContent);
      button.textContent = 'Copied';
    } catch {
      const range = document.createRange();
      range.selectNodeContents(code);
      const selection = window.getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
      button.textContent = 'Text selected';
    }
    setTimeout(() => { button.textContent = 'Copy'; }, 1800);
  });
});
