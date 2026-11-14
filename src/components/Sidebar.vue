<script setup lang="ts">
import { Icon } from "@iconify/vue";

defineProps<{ page: string; theme: string }>();
defineEmits<{ (e: "nav", id: string): void; (e: "theme"): void }>();

const items = [
  { id: "home", icon: "line-md:home", label: "首页" },
  { id: "perf", icon: "line-md:gauge", label: "性能" },
  { id: "peripherals", icon: "line-md:monitor", label: "外设" },
  { id: "materials", icon: "line-md:folder", label: "素材" },
  { id: "settings", icon: "line-md:cog", label: "设置" },
];
</script>

<template>
  <aside class="side">
    <div class="side-nav">
      <el-tooltip
        v-for="it in items"
        :key="it.id"
        :content="it.label"
        placement="right"
        :show-after="260"
      >
        <button
          type="button"
          class="side-btn"
          :class="{ on: page === it.id }"
          @click="$emit('nav', it.id)"
        >
          <Icon :icon="it.icon" />
          <span>{{ it.label }}</span>
          <i class="side-ind" aria-hidden="true"></i>
        </button>
      </el-tooltip>
    </div>
    <el-tooltip :content="theme === 'dark' ? '切换浅色' : '切换深色'" placement="right" :show-after="260">
      <button type="button" class="side-btn side-theme" @click="$emit('theme')">
        <Icon icon="line-md:light-dark" />
        <span>{{ theme === "dark" ? "浅色" : "深色" }}</span>
      </button>
    </el-tooltip>
  </aside>
</template>
