<script setup lang="ts">
import { ref } from 'vue';
import { useConnectionStore, type ConnectionProfile } from '@/stores/connection';
import { useToast } from '@/composables/useToast';
import Button from '@/components/ui/Button.vue';
import Icon from '@/components/ui/Icon.vue';

const conn = useConnectionStore();
const { push } = useToast();

defineProps<{ open: boolean }>();
defineEmits<{ close: [] }>();

const draftLabel = ref('');
const draftUrl = ref('');
const draftToken = ref('');
const editingId = ref<string | null>(null);

function reset() {
  draftLabel.value = '';
  draftUrl.value = '';
  draftToken.value = '';
  editingId.value = null;
}

function startEdit(p: ConnectionProfile) {
  draftLabel.value = p.label;
  draftUrl.value = p.baseUrl;
  draftToken.value = p.bearerToken ?? '';
  editingId.value = p.id;
}

function save() {
  try {
    if (editingId.value) {
      conn.updateProfile(editingId.value, {
        label: draftLabel.value,
        baseUrl: draftUrl.value,
        bearerToken: draftToken.value,
      });
      push('success', 'profile updated');
    } else {
      conn.addProfile({
        label: draftLabel.value || draftUrl.value,
        baseUrl: draftUrl.value,
        bearerToken: draftToken.value,
      });
      push('success', 'profile added');
    }
    reset();
  } catch (e) {
    push('error', e instanceof Error ? e.message : String(e), 6000);
  }
}
</script>

<template>
  <Transition name="drawer">
    <aside v-if="open" class="drawer" role="dialog" aria-label="Connection profiles">
      <header class="drawer-head">
        <div>
          <div class="kicker">CONNECTIONS</div>
          <h2>Relays</h2>
        </div>
        <button class="drawer-close" @click="$emit('close')" aria-label="Close">
          <Icon name="close" :size="18" />
        </button>
      </header>

      <section class="drawer-body">
        <div v-if="!conn.profiles.length" class="hint">
          No relays registered. Add the first one below; we store the profile + token in this
          browser's localStorage.
        </div>

        <ul v-else class="profile-list">
          <li
            v-for="p in conn.profiles"
            :key="p.id"
            :class="{ 'is-active': p.id === conn.activeId }"
          >
            <button class="profile-row" @click="conn.setActive(p.id)">
              <div class="profile-meta">
                <strong>{{ p.label }}</strong>
                <span>{{ p.baseUrl }}</span>
              </div>
              <span v-if="p.id === conn.activeId" class="profile-active">ACTIVE</span>
            </button>
            <div class="profile-actions">
              <Button small variant="ghost" @click="startEdit(p)">edit</Button>
              <Button small variant="danger" @click="conn.removeProfile(p.id)">remove</Button>
            </div>
          </li>
        </ul>

        <div class="divider" />

        <form class="profile-form" @submit.prevent="save">
          <div class="kicker">{{ editingId ? 'EDIT PROFILE' : 'ADD PROFILE' }}</div>
          <label>
            <span>Label</span>
            <input v-model="draftLabel" placeholder="staging" autocomplete="off" />
          </label>
          <label>
            <span>Base URL</span>
            <input v-model="draftUrl" placeholder="http://localhost:8080" required />
          </label>
          <label>
            <span>Bearer token (optional)</span>
            <input v-model="draftToken" type="password" autocomplete="off" />
          </label>
          <div class="form-actions">
            <Button v-if="editingId" variant="ghost" @click="reset" type="button">Cancel</Button>
            <Button variant="primary" type="submit">{{ editingId ? 'Save changes' : 'Add relay' }}</Button>
          </div>
        </form>

        <p class="warning">
          Tokens are stored in this browser's localStorage. Clear them when sharing the device.
        </p>
      </section>
    </aside>
  </Transition>
  <div v-if="open" class="drawer-overlay" @click="$emit('close')" />
</template>

<style scoped>
.drawer {
  position: fixed;
  top: 0;
  right: 0;
  bottom: 0;
  width: min(420px, 100%);
  background: var(--paper);
  border-left: 1px solid var(--chalk-hi);
  z-index: 30;
  display: flex;
  flex-direction: column;
  box-shadow: -8px 0 24px rgba(20, 32, 46, 0.18);
}
.drawer-overlay {
  position: fixed;
  inset: 0;
  background: rgba(20, 32, 46, 0.4);
  z-index: 25;
}
.drawer-head {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  padding: var(--s-5);
  border-bottom: 1px solid var(--chalk);
}
.drawer-head h2 {
  font-family: var(--font-display);
  font-size: 28px;
  line-height: 1;
  letter-spacing: -0.01em;
}
.kicker {
  font-family: var(--font-mono);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.18em;
  text-transform: uppercase;
  color: var(--ink-faint);
  margin-bottom: 4px;
}
.drawer-close {
  width: 32px;
  height: 32px;
  border: 1px solid var(--chalk-hi);
  display: inline-flex;
  align-items: center;
  justify-content: center;
}
.drawer-body {
  flex: 1;
  overflow-y: auto;
  padding: var(--s-5);
}
.hint {
  font-size: 13px;
  color: var(--ink-muted);
  background: var(--chalk-lo);
  border: 1px solid var(--chalk-hi);
  padding: var(--s-3);
  margin-bottom: var(--s-4);
}
.profile-list {
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
  margin-bottom: var(--s-4);
}
.profile-list li {
  border: 1px solid var(--chalk-hi);
  background: var(--paper-hi);
}
.profile-list li.is-active {
  border-color: var(--tally);
}
.profile-row {
  display: flex;
  width: 100%;
  align-items: center;
  justify-content: space-between;
  padding: var(--s-3) var(--s-4);
  text-align: left;
}
.profile-meta strong {
  font-family: var(--font-display);
  font-size: 18px;
  display: block;
  letter-spacing: -0.01em;
}
.profile-meta span {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
}
.profile-active {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.18em;
  color: var(--tally-deep);
}
.profile-actions {
  display: flex;
  gap: var(--s-2);
  padding: 0 var(--s-4) var(--s-3);
}
.divider {
  height: 1px;
  background: var(--chalk);
  margin: var(--s-4) 0;
}
.profile-form {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.profile-form label {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.profile-form span {
  font-family: var(--font-mono);
  font-size: 11px;
  letter-spacing: 0.1em;
  text-transform: uppercase;
  color: var(--ink-faint);
}
.profile-form input {
  border: 1px solid var(--chalk-hi);
  background: var(--paper-hi);
  padding: 7px 10px;
  font-size: 13px;
  font-family: var(--font-mono);
}
.form-actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--s-2);
}
.warning {
  margin-top: var(--s-4);
  padding: var(--s-3);
  background: rgba(217, 119, 6, 0.08);
  border: 1px solid var(--warn);
  font-size: 12px;
  color: var(--ink-light);
}

.drawer-enter-active,
.drawer-leave-active {
  transition: transform 0.18s ease;
}
.drawer-enter-from,
.drawer-leave-to {
  transform: translateX(100%);
}
</style>
