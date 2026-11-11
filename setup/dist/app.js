// Text only ever goes in through textContent (h() makes text nodes), never innerHTML.
function h(tag, props = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (key.startsWith('on')) el.addEventListener(key.slice(2), value);
    else el.setAttribute(key, value);
  }
  el.append(...children.filter((c) => c != null));
  return el;
}

const invoke = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args);

const TEXT = {
  en: {
    install: 'Install',
    update: 'Update',
    cancel: 'Cancel',
    version: 'Version {version}',
    installed: 'Installed: {version}',
    folder: 'Folder: {folder}',
    installing: 'Installing {name}…',
    done: '{name} is installed',
    open: 'Open {name}',
    close: 'Close',
    failed: "The installation didn't finish.",
    exitCode: 'Exit code: {code}',
    retry: 'Try again',
  },
  'pt-BR': {
    install: 'Instalar',
    update: 'Atualizar',
    cancel: 'Cancelar',
    version: 'Versão {version}',
    installed: 'Instalada: {version}',
    folder: 'Pasta: {folder}',
    installing: 'Instalando o {name}…',
    done: 'O {name} foi instalado',
    open: 'Abrir o {name}',
    close: 'Fechar',
    failed: 'A instalação não terminou.',
    exitCode: 'Código de saída: {code}',
    retry: 'Tentar de novo',
  },
};

const first = [...(navigator.languages ?? []), navigator.language].find(Boolean) ?? 'en';
const lang = /^pt/i.test(first) ? 'pt-BR' : 'en';
document.documentElement.lang = lang;
const t = (key, vars = {}) => TEXT[lang][key].replace(/\{(\w+)\}/g, (_, k) => vars[k]);

// Release builds only: the page isn't a browser, so no right-click menu.
addEventListener('contextmenu', (e) => e.preventDefault());

const card = document.getElementById('card');
const live = document.getElementById('live');
let info;
let busy = false;

const mark = () => h('img', { class: 'mark', src: 'http://jayi.localhost/mark.png', alt: '' });
const button = (label, onclick, secondary) =>
  h('button', { type: 'button', class: secondary ? 'btn btn-secondary' : 'btn', onclick }, label);

function setBusy(value) {
  busy = value;
  for (const b of card.querySelectorAll('button')) b.disabled = value;
}

async function install() {
  if (busy) return;
  setBusy(true);
  show('installing');
  try {
    const code = await invoke('install');
    if (code !== 0) show('error', { code });
    else if (info.passive) await invoke('close');
    else show('done');
  } catch (e) {
    show('error', { message: String(e) });
  } finally {
    setBusy(false);
  }
}

const close = () => invoke('close');
const openApp = () => invoke('open').catch((e) => show('error', { message: String(e) }));

// Each screen: [heading text, extra body lines, buttons, element to focus ('primary' or 'heading')].
function screens(extra) {
  const name = info.name;
  const lines = (...items) => items.filter(Boolean).map((text) => h('p', { class: 'muted' }, text));
  return {
    confirm: {
      title: name,
      body: [
        ...lines(t('version', { version: info.version }), info.installed && t('installed', { version: info.installed })),
        h('p', { class: 'folder' }, t('folder', { folder: info.folder })),
      ],
      buttons: [
        button(info.installed ? t('update') : t('install'), install),
        button(t('cancel'), close, true),
      ],
    },
    installing: {
      title: name,
      status: t('installing', { name }),
      body: [h('div', { class: 'progress', 'aria-hidden': 'true' }, h('span'))],
      buttons: [],
    },
    done: {
      title: t('done', { name }),
      body: [],
      buttons: [button(t('open', { name }), openApp), button(t('close'), close, true)],
    },
    error: {
      title: t('failed'),
      body: [
        h('p', { class: 'error', role: 'alert' }, extra.code != null ? t('exitCode', { code: extra.code }) : extra.message),
      ],
      buttons: [button(t('retry'), install), button(t('close'), close, true)],
    },
  };
}

function show(id, extra = {}) {
  const s = screens(extra)[id];
  const heading = h('h1', { tabindex: '-1' }, s.title);
  const status = s.status ? h('p', { class: 'muted', tabindex: '-1' }, s.status) : null;
  card.replaceChildren(
    h(
      'section',
      { class: 'screen' },
      h('div', { class: 'body' }, mark(), heading, status, ...s.body),
      h('div', { class: 'actions' }, ...s.buttons),
    ),
  );
  live.textContent = s.status ?? s.title;
  (card.querySelector('button') ?? status ?? heading).focus();
  if (busy) setBusy(true);
}

try {
  const [name, version, installed, folder, passive] = await invoke('info');
  info = { name, version, installed, folder, passive };
  document.title = name;
  if (passive) install();
  else show('confirm');
} catch (e) {
  info ??= { name: '', version: '', installed: null, folder: '', passive: false }; // the error screen needs it
  show('error', { message: String(e) });
}
