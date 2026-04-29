import { createRouter, createWebHashHistory, type RouteRecordRaw } from 'vue-router';

// Hash-based history so the dist/ deploys behind any static host without
// needing rewrite rules. Operators on a host that supports clean URLs can
// override at build time.
const routes: RouteRecordRaw[] = [
  {
    path: '/',
    redirect: '/dashboard',
  },
  {
    path: '/dashboard',
    name: 'dashboard',
    component: () => import('@/views/Dashboard.vue'),
    meta: { rail: 'operations', label: 'Dashboard', icon: 'grid' },
  },
  {
    path: '/streams',
    name: 'streams',
    component: () => import('@/views/Streams.vue'),
    meta: { rail: 'operations', label: 'Streams', icon: 'streams' },
  },
  {
    path: '/streams/:name',
    name: 'stream-detail',
    component: () => import('@/views/StreamDetail.vue'),
    meta: { rail: null, label: 'Stream Detail' },
  },
  {
    path: '/recordings',
    name: 'recordings',
    component: () => import('@/views/Recordings.vue'),
    meta: { rail: 'operations', label: 'Recordings', icon: 'rec' },
  },
  {
    path: '/dvr',
    name: 'dvr',
    component: () => import('@/views/Dvr.vue'),
    meta: { rail: 'operations', label: 'DVR', icon: 'play' },
  },
  {
    path: '/ingest',
    name: 'ingest',
    component: () => import('@/views/Ingest.vue'),
    meta: { rail: 'pipeline', label: 'Ingest', icon: 'ingest' },
  },
  {
    path: '/filters',
    name: 'filters',
    component: () => import('@/views/Filters.vue'),
    meta: { rail: 'pipeline', label: 'Filters', icon: 'filters' },
  },
  {
    path: '/filters/:index',
    name: 'filter-detail',
    component: () => import('@/views/FilterDetail.vue'),
    meta: { rail: null, label: 'Filter Detail' },
  },
  {
    path: '/transcode',
    name: 'transcode',
    component: () => import('@/views/Transcode.vue'),
    meta: { rail: 'pipeline', label: 'Transcode', icon: 'transcode' },
  },
  {
    path: '/agents',
    name: 'agents',
    component: () => import('@/views/Agents.vue'),
    meta: { rail: 'pipeline', label: 'Agents', icon: 'agents' },
  },
  {
    path: '/egress',
    name: 'egress',
    component: () => import('@/views/Egress.vue'),
    meta: { rail: 'pipeline', label: 'Egress', icon: 'egress' },
  },
  {
    path: '/cluster',
    name: 'cluster',
    component: () => import('@/views/Cluster.vue'),
    meta: { rail: 'infrastructure', label: 'Cluster', icon: 'cluster' },
  },
  {
    path: '/mesh',
    name: 'mesh',
    component: () => import('@/views/Mesh.vue'),
    meta: { rail: 'infrastructure', label: 'Mesh', icon: 'mesh' },
  },
  {
    path: '/federation',
    name: 'federation',
    component: () => import('@/views/Federation.vue'),
    meta: { rail: 'infrastructure', label: 'Federation', icon: 'federation' },
  },
  {
    path: '/auth',
    name: 'auth',
    component: () => import('@/views/Auth.vue'),
    meta: { rail: 'identity', label: 'Auth', icon: 'lock' },
  },
  {
    path: '/provenance',
    name: 'provenance',
    component: () => import('@/views/Provenance.vue'),
    meta: { rail: 'identity', label: 'Provenance', icon: 'shield' },
  },
  {
    path: '/observability',
    name: 'observability',
    component: () => import('@/views/Observability.vue'),
    meta: { rail: 'system', label: 'Observability', icon: 'chart' },
  },
  {
    path: '/logs',
    name: 'logs',
    component: () => import('@/views/Logs.vue'),
    meta: { rail: 'system', label: 'Logs', icon: 'logs' },
  },
  {
    path: '/settings',
    name: 'settings',
    component: () => import('@/views/Settings.vue'),
    meta: { rail: 'system', label: 'Settings', icon: 'gear' },
  },
];

export const router = createRouter({
  history: createWebHashHistory(),
  routes,
});

/** Rail section labels in display order. */
export const RAIL_SECTIONS: Array<{ id: string; label: string }> = [
  { id: 'operations', label: 'Operations' },
  { id: 'pipeline', label: 'Pipeline' },
  { id: 'infrastructure', label: 'Infrastructure' },
  { id: 'identity', label: 'Identity' },
  { id: 'system', label: 'System' },
];
