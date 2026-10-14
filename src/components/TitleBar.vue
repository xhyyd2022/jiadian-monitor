<script setup lang="ts">
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Icon } from "@iconify/vue";

const appWindow = getCurrentWindow();
defineEmits<{ (e: "close"): void }>();
</script>

<template>
  <header class="titlebar" data-tauri-drag-region>
    <span class="tb-title" data-tauri-drag-region>jiaidian-monitor</span>
    <div class="tb-btns">
      <button class="tb-btn" @click="appWindow.minimize()" title="最小化">
        <Icon icon="line-md:minus" />
      </button>
      <button class="tb-btn" @click="appWindow.toggleMaximize()" title="最大化">
        <Icon icon="line-md:square" />
      </button>
      <button class="tb-btn tb-close" @click="$emit('close')" title="关闭">
        <Icon icon="line-md:close" />
      </button>
    </div>
  </header>
</template>
