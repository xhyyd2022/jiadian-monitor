<script setup lang="ts">
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { Icon } from "@iconify/vue";
import GUIDE from "./material_prompt.md?raw";

/// 在线 AI 站点（要换站点就改这一行）
const AI_URL = "https://chat.deepseek.com/";

const copied = ref(false);
async function copyPrompt() {
  try {
    await navigator.clipboard.writeText(GUIDE);
    copied.value = true;
    setTimeout(() => (copied.value = false), 1500);
  } catch {
    copied.value = false;
  }
}
async function openAi() {
  try {
    await invoke("open_url", { url: AI_URL });
  } catch {
    /* 打不开就让用户自己开浏览器 */
  }
}
</script>

<template>
  <div>
    <div class="card">
      <h2><Icon icon="line-md:folder" />素材添加</h2>
      <p class="hint">
        主题 = <code>assets/</code> 下一个文件夹（名即主题名），主页面 <code>index.html</code>，图片与它同层。
      </p>
      <div class="btns two">
        <button class="primary" @click="copyPrompt">{{ copied ? "已复制" : "复制" }}</button>
        <button @click="openAi">一键打开在线 AI</button>
      </div>
    </div>

    <div class="card">
      <h2><Icon icon="line-md:list" />使用步骤</h2>
      <ol class="steps">
        <li>复制提示词，粘给 AI。</li>
        <li>说明配色、布局、要显示的指标。</li>
        <li>HTML 与图片放进 <code>assets/主题名/</code>，主页面命名 <code>index.html</code>。</li>
        <li>回「首页」选中主题点「确定」下发；或点「导入素材」自动建文件夹。</li>
      </ol>
    </div>

    <div class="card">
      <h2><Icon icon="line-md:file-import" />提示词预览</h2>
      <pre class="guide">{{ GUIDE }}</pre>
    </div>
  </div>
</template>
