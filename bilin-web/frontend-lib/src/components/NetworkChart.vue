<template>
  <div
    :class="monochrome ? 'chart-desktop' : 'bg-gradient-to-br from-blue-50 to-indigo-100 dark:from-blue-900/20 dark:to-indigo-800/20 rounded-xl p-4 border border-blue-200 dark:border-blue-700 shadow-md hover:shadow-lg transition-all duration-300'">
    <dl v-if="monochrome" class="chart-summary">
      <div><dt><span aria-hidden="true" />{{ t('upload') }}</dt><dd>{{ currentUpload }}<small>/s</small></dd></div>
      <div><dt><span aria-hidden="true" />{{ t('download') }}</dt><dd>{{ currentDownload }}<small>/s</small></dd></div>
    </dl>
    <div v-else class="flex items-center justify-center mb-3">
      <div class="flex gap-2 text-sm">
        <span class="flex items-center gap-1 w-32">
          <div class="w-2 h-2 bg-green-500 rounded-full"></div>
          <span class="text-green-600 dark:text-green-400 truncate">{{ t('upload') }}: {{ currentUpload }}/s</span>
        </span>
        <span class="flex items-center gap-1 w-32">
          <div class="w-2 h-2 bg-blue-500 rounded-full"></div>
          <span class="text-blue-600 dark:text-blue-400 truncate">{{ t('download') }}: {{ currentDownload }}/s</span>
        </span>
      </div>
    </div>
    <div :class="monochrome ? 'chart-canvas' : 'h-32'">
      <canvas ref="chartCanvas" role="img" :aria-label="t('desktop.traffic')"></canvas>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch, nextTick } from 'vue'
import {
  Chart as ChartJS,
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  LineController,
  Title,
  Tooltip,
  Legend,
  Filler
} from 'chart.js'
import { useI18n } from 'vue-i18n';

const { t } = useI18n()

// 注册Chart.js组件
ChartJS.register(
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  LineController,
  Title,
  Tooltip,
  Legend,
  Filler
)

interface Props {
  uploadRate: string
  downloadRate: string
  monochrome?: boolean
}

const props = defineProps<Props>()

const chartCanvas = ref<HTMLCanvasElement>()
let chart: ChartJS | null = null
let updateTimer: number | null = null

// 存储历史数据，最多保存30个数据点（1分钟历史）
const maxDataPoints = 120
const uploadHistory: number[] = []
const downloadHistory: number[] = []
const timeLabels: string[] = []

const currentUpload = ref('0')
const currentDownload = ref('0')

// 将带单位的速率字符串转换为字节数
function parseRateToBytes(rateStr: string): number {
  if (!rateStr || rateStr === '0') return 0

  const match = rateStr.match(/([0-9.]+)\s*([KMGT]?i?B)/i)
  if (!match) return 0

  const value = parseFloat(match[1])
  const unit = match[2].toUpperCase()

  const multipliers: { [key: string]: number } = {
    'B': 1,
    'KB': 1000,
    'KIB': 1024,
    'MB': 1000000,
    'MIB': 1024 * 1024,
    'GB': 1000000000,
    'GIB': 1024 * 1024 * 1024,
    'TB': 1000000000000,
    'TIB': 1024 * 1024 * 1024 * 1024
  }

  return value * (multipliers[unit] || 1)
}

// 格式化字节为可读格式
function formatBytes(bytes: number): string {
  if (bytes < 1) return bytes.toFixed(1) + ' B'

  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.floor(Math.log(bytes) / Math.log(k))

  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i]
}

// 更新数据
function updateData() {
  const uploadBytes = parseRateToBytes(props.uploadRate)
  const downloadBytes = parseRateToBytes(props.downloadRate)

  currentUpload.value = formatBytes(uploadBytes)
  currentDownload.value = formatBytes(downloadBytes)

  // 添加新数据点
  uploadHistory.push(uploadBytes)
  downloadHistory.push(downloadBytes)

  // 生成时间标签
  const now = new Date()
  const timeStr = now.toLocaleTimeString('zh-CN', {
    hour12: false,
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit'
  })
  timeLabels.push(timeStr)

  // 保持数据点数量不超过最大值
  if (uploadHistory.length > maxDataPoints) {
    uploadHistory.shift()
    downloadHistory.shift()
    timeLabels.shift()
  }

  // 更新图表
  if (chart) {
    chart.data.labels = timeLabels
    chart.data.datasets[0].data = uploadHistory
    chart.data.datasets[1].data = downloadHistory
    chart.update('none')
  }
}

// 初始化图表
function initChart() {
  if (!chartCanvas.value) return

  const ctx = chartCanvas.value.getContext('2d')
  if (!ctx) return

  chart = new ChartJS(ctx, {
    type: 'line',
    data: {
      labels: timeLabels,
      datasets: [
        {
          label: t('upload'),
          data: uploadHistory,
          borderColor: props.monochrome ? '#1e1e1e' : 'rgb(34, 197, 94)',
          backgroundColor: props.monochrome ? 'rgba(0, 0, 0, 0.055)' : 'rgba(34, 197, 94, 0.1)',
          borderWidth: 2,
          fill: true,
          tension: 0.4,
          pointRadius: 0,
          pointHoverRadius: 4
        },
        {
          label: t('download'),
          data: downloadHistory,
          borderColor: props.monochrome ? '#7a7a7a' : 'rgb(59, 130, 246)',
          backgroundColor: 'rgba(59, 130, 246, 0.1)',
          borderDash: props.monochrome ? [5, 3] : undefined,
          borderWidth: 2,
          fill: !props.monochrome,
          tension: 0.4,
          pointRadius: 0,
          pointHoverRadius: 4
        }
      ]
    },
    options: {
      responsive: true,
      maintainAspectRatio: false,
      interaction: {
        intersect: false,
        mode: 'index'
      },
      plugins: {
        legend: {
          display: false
        },
        tooltip: {
          callbacks: {
            label: function (context: any) {
              const value = context.parsed.y
              return `${context.dataset.label}: ${formatBytes(value)}/s`
            }
          }
        }
      },
      scales: {
        x: {
          display: true,
          grid: {
            display: false
          },
          ticks: {
            color: props.monochrome ? '#626262' : undefined,
            maxTicksLimit: props.monochrome ? 5 : 3,
            font: {
              size: props.monochrome ? 10 : 8
            }
          }
        },
        y: {
          display: true,
          beginAtZero: true,
          min: 0,
          grid: {
            color: props.monochrome ? '#ebebeb' : 'rgba(0, 0, 0, 0.1)'
          },
          ticks: {
            color: props.monochrome ? '#626262' : undefined,
            maxTicksLimit: props.monochrome ? 5 : undefined,
            callback: function (value: any) {
              return formatBytes(value as number)
            },
            font: {
              size: props.monochrome ? 10 : 8
            },
          },
        }
      },
      animation: {
        duration: 10
      }
    }
  })
}

// 监听props变化
watch([() => props.uploadRate, () => props.downloadRate], () => {
  if (!props.monochrome) updateData()
}, { immediate: true })

onMounted(async () => {
  // add initial point
  const now = new Date();
  for (let i = 0; !props.monochrome && i < maxDataPoints; i++) {
    let date = new Date(now.getTime() - (maxDataPoints - i) * 2000)
    const timeStr = date.toLocaleTimeString(navigator.language, {
      hour12: false,
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit'
    })
    uploadHistory.push(0)
    downloadHistory.push(0)
    timeLabels.push(timeStr)
  }

  await nextTick()
  initChart()
  updateData()

  // 启动定时器，每2秒更新一次图表
  updateTimer = window.setInterval(() => {
    updateData()
  }, 2000)
})

onUnmounted(() => {
  if (chart) {
    chart.destroy()
  }
  if (updateTimer) {
    clearInterval(updateTimer)
  }
})
</script>

<style scoped>
.chart-summary { display: flex; flex-wrap: wrap; gap: 16px 30px; margin: 0 0 14px; }
.chart-summary dt { display: flex; gap: 7px; align-items: center; color: #565656; font-size: 12px; }
.chart-summary dt span { width: 16px; border-top: 2px solid #202020; }
.chart-summary > div:nth-child(2) dt span { border-top: 2px dashed #777; }
.chart-summary dd { margin: 2px 0 0; color: #1a1a1a; font-size: 20px; font-family: var(--desk-mono, monospace); font-weight: 600; }
.chart-summary dd small { color: #626262; font-size: 11px; margin-left: 3px; }
.chart-canvas { height: clamp(122px, 25vh, 188px); }
</style>
