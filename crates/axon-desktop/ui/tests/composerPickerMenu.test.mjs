/*
 * 输入坞模型选择器的菜单布局：模型与推理等级并排两列。
 *
 * 这条形态来自实际手感：两段叠成一列时，档位被十来个模型项压在菜单最下面，
 * 每次改思考强度都要先把清单滚到底。这里钉住「档位不在滚动容器里、且与模型
 * 同屏可见」，以及模型清单为空时档位照常可选。
 */

import React from 'react';
import test from 'node:test';
import assert from 'node:assert/strict';
import { renderToStaticMarkup } from 'react-dom/server';
import { ComposerPickerMenu } from '../src/components/conversation/ComposerPicker.tsx';

const MODELS = ['glm-5', 'deepseek-v4.1-flash'];
const EFFORTS = ['None', 'Minimal', 'Low', 'Medium', 'High', 'Xhigh', 'Max'];

function render(overrides = {}) {
  return renderToStaticMarkup(
    React.createElement(ComposerPickerMenu, {
      model: 'glm-5',
      models: MODELS,
      effort: 'high',
      onModelChange: () => undefined,
      onReasoningEffortChange: () => undefined,
      ...overrides,
    }),
  );
}

test('模型列在左、推理等级列在右，两列同屏出现', () => {
  const html = render();
  const model = html.indexOf('data-column="model"');
  const effort = html.indexOf('data-column="effort"');
  assert.ok(model >= 0, '必须有模型列');
  assert.ok(effort > model, '推理等级列必须独立成列并排在模型列之后');
  assert.match(html, />模型</);
  assert.match(html, />推理等级</);
});

test('推理等级列不进滚动容器，七档展开即全部可选', () => {
  const html = render();
  const effortStart = html.indexOf('data-column="effort"');
  assert.ok(effortStart >= 0, '必须有推理等级列');
  assert.doesNotMatch(
    html.slice(effortStart),
    /overflow-y-auto/,
    '档位列不能被卷进滚动容器：那正是「要拉到最下方才能选思考强度」的成因',
  );
  for (const label of EFFORTS) {
    assert.match(html, new RegExp(`>${label}<`), `缺少档位 ${label}`);
  }
});

test('滚动只发生在模型列内', () => {
  const html = render();
  const modelStart = html.indexOf('data-column="model"');
  const effortStart = html.indexOf('data-column="effort"');
  assert.match(
    html.slice(modelStart, effortStart),
    /overflow-y-auto/,
    '模型清单会随配置变长，滚动必须由模型列自己承担',
  );
});

test('模型清单为空时给出提示，档位仍然整段可选', () => {
  const html = render({ models: [] });
  assert.match(html, /配置里还没有可用模型/);
  const effortStart = html.indexOf('data-column="effort"');
  assert.doesNotMatch(html.slice(effortStart), /overflow-y-auto/);
  for (const label of EFFORTS) {
    assert.match(html, new RegExp(`>${label}<`), `缺少档位 ${label}`);
  }
});
