// 输入区：自适应 textarea + 对话式发送（Enter 直发）+ 发送旁的分析/规划模型选择
// + 自动分级/复杂度 + 技能多选 + 运行中控制（调整方向 / 追加任务 / 停止）。

import { h, cn, button, badge, spinner, openPopover, toast, kbd } from './ui.js';
import { modelSelectButton, requestSummary } from './model-select.js';
import { state } from './store.js';

export function createComposer({ container, onSubmit, onControl, onCancel }) {
  let running = false;
  let sendingControl = false;
  let cancelRequested = false;

  const textarea = h('textarea', {
    'data-component': 'textarea',
    class: 'composer-input',
    placeholder: '发送消息，描述你的任务…（Enter 发送，Shift+Enter 换行）',
    maxlength: '100000',
    rows: '2',
    'aria-label': '任务内容',
  });
  const sendLabel = h('span', {}, '发送');
  const sendBtn = button(
    running ? spinner('0.8rem') : null, sendLabel,
    { type: 'submit', 'data-variant': 'default', 'data-size': 'lg', class: 'composer-send', disabled: true });
  const cancelBtn = button('■ 停止执行', {
    type: 'button', 'data-variant': 'destructive', 'data-size': 'sm', class: 'composer-cancel', hidden: true,
    onclick: () => { cancelRequested = true; update(); onCancel?.(); },
  });
  const modeSelect = h('select', {
    class: 'composer-mode',
    'aria-label': '运行中发送模式',
    title: '任务运行中发送内容的方式',
  },
    h('option', { value: 'steer' }, '调整方向 · 下一轮生效'),
    h('option', { value: 'follow-up' }, '追加任务 · 完成后处理'),
  );
  modeSelect.hidden = true;

  const automatic = h('input', { type: 'checkbox', class: 'composer-automatic', checked: true, id: 'automatic-toggle' });
  const complexity = h('input', { type: 'range', min: '0', max: '10', value: '4', id: 'complexity-range', 'aria-label': '任务复杂度' });
  const complexityValue = h('output', { for: 'complexity-range' }, '4');
  automatic.addEventListener('change', () => {
    state.request.automatic = automatic.checked;
    complexity.disabled = automatic.checked;
  });
  complexity.addEventListener('input', () => {
    state.request.complexity = Number(complexity.value);
    state.request.automatic = false;
    automatic.checked = false;
    complexity.disabled = false;
    complexityValue.textContent = complexity.value;
  });

  let skillsBtn;
  const skillsBtnLabel = h('span', {}, `技能 ${state.selectedSkills.size}`);
  skillsBtn = button(skillsBtnLabel, {
    'data-variant': 'ghost', 'data-size': 'sm',
    onclick: (event) => {
      const panel = h('div', { class: 'skills-pop' });
      if (!state.skills.length) panel.append(h('p', { class: 'text-ui-sm text-muted' }, '尚未发现技能（SKILL.md）。'));
      for (const skill of state.skills) {
        const name = String(skill.name || skill.id || '').replace(/^skill:/, '');
        if (!name) continue;
        const box = h('input', { type: 'checkbox', id: `skill-${name}` });
        box.checked = state.selectedSkills.has(name);
        box.addEventListener('change', () => {
          if (box.checked) state.selectedSkills.add(name); else state.selectedSkills.delete(name);
          skillsBtnLabel.textContent = `技能 ${state.selectedSkills.size}`;
        });
        panel.append(h('label', { class: 'skills-pop-row', for: `skill-${name}` }, box,
          h('span', { class: 'min-w-0' },
            h('span', { style: { display: 'block', fontWeight: '500' } }, name),
            skill.description ? h('span', { class: 'text-ui-xs text-subtle', style: { display: 'block' } }, String(skill.description).slice(0, 64)) : null,
          )));
      }
      openPopover(event.currentTarget, panel, { align: 'start' });
    },
  });

  const modelChip = modelSelectButton(state, {});

  function autoResize() {
    textarea.style.height = 'auto';
    textarea.style.height = `${Math.min(textarea.scrollHeight, 220)}px`;
  }
  textarea.addEventListener('input', () => { autoResize(); update(); });
  textarea.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      // 单击 Enter 直接触发提交（Shift+Enter 换行）；组合键由全局快捷键处理。
      event.preventDefault();
      submitFlow();
    }
  });

  async function submitFlow() {
    const prompt = textarea.value.trim();
    if (!prompt) return;
    if (running) {
      if (!state.task?.id || sendingControl || cancelRequested) return;
      sendingControl = true;
      update();
      const kind = modeSelect.value === 'follow-up' ? 'follow-up' : 'steer';
      try {
        const queued = await onControl(prompt, kind);
        toast(`${kind === 'steer' ? '已排队调整方向' : '已排队追加任务'} · 队列序号 ${queued?.sequence ?? ''}`);
        textarea.value = '';
        autoResize();
      } catch (error) {
        toast(`任务调整失败：${error.message}`, 'error');
      } finally {
        sendingControl = false;
        update();
      }
      return;
    }
    const value = textarea.value;
    textarea.value = '';
    autoResize();
    update();
    const failure = await onSubmit(value);
    if (failure) {
      textarea.value = value;
      autoResize();
      update();
    }
  }

  function update() {
    const hasText = Boolean(textarea.value.trim());
    sendBtn.disabled = !hasText || (running && (!state.task?.id || sendingControl || cancelRequested));
    sendLabel.textContent = running
      ? (modeSelect.value === 'follow-up' ? '追加任务' : '调整方向')
      : '发送';
    sendBtn.setAttribute('data-variant', running ? 'secondary' : 'default');
    modeSelect.hidden = !running;
    cancelBtn.hidden = !running;
    if (cancelRequested) cancelBtn.textContent = '正在取消…';
    else cancelBtn.textContent = '■ 停止执行';
  }

  const form = h('form', {
    class: 'composer',
    onsubmit: (event) => { event.preventDefault(); submitFlow(); },
  },
    h('div', { class: 'composer-shell', 'data-beam': 'ocean' },
      textarea,
      h('div', { class: 'composer-bar' },
        h('label', { class: 'composer-auto', title: '让路由器从任务描述推断所需智能层级' },
          automatic, h('span', {}, '自动分级'),
          complexity, complexityValue),
        h('span', { style: { flex: '1' } }),
        skillsBtn,
        modeSelect,
        cancelBtn,
        modelChip,
        sendBtn,
      ),
      h('div', { class: 'composer-foot' },
        h('span', { class: 'text-ui-xs text-subtle' }, '当前选择：'),
        h('span', { class: 'text-ui-xs mono composer-foot-summary' }, requestSummary(state)),
        h('span', { class: 'text-ui-xs text-subtle' }, '· '),
        h('span', { class: 'text-ui-xs text-subtle' }, 'Shift+Enter 换行'),
      ),
    ),
  );
  container.append(form);
  document.addEventListener('model-changed', () => {
    form.querySelector('.composer-foot-summary').textContent = requestSummary(state);
  });
  autoResize();

  return {
    focus: () => textarea.focus(),
    setRunning(value) {
      running = value;
      if (!value) { sendingControl = false; cancelRequested = false; }
      update();
    },
    get value() { return textarea.value.trim(); },
    update,
    get cancelRequested() { return cancelRequested; },
  };
}
