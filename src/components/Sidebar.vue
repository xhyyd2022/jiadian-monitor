<script setup lang="ts">
import { Icon } from "@iconify/vue";

defineProps<{ page: string; theme: string }>();
defineEmits<{ (e: "nav", id: string): void; (e: "theme"): void }>();

const items = [
  { id: "home", icon: "line-md:home", label: "首页" },
  { id: "perf", icon: "line-md:gauge", label: "性能" },
  { id: "materials", icon: "line-md:folder", label: "素材" },
  { id: "settings", icon: "line-md:cog", label: "设置" },
];
</script>

<template>
  <aside class="side">
    <button
      v-for="it in items"
      :key="it.id"
      :class="{ on: page === it.id }"
      @click="$emit('nav', it.id)"
    >
      <Icon :icon="it.icon" /><span>{{ it.label }}</span>
    </button>
    <button class="theme" @click="$emit('theme')">
      <Icon icon="line-md:light-dark" /><span>{{ theme === "dark" ? "浅色" : "深色" }}</span>
    </button>
  </aside>
</template>
