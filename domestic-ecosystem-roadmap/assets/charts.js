(function () {
  var root = getComputedStyle(document.documentElement);
  var accent = root.getPropertyValue('--accent').trim() || '#6c5ce7';
  var accent2 = root.getPropertyValue('--accent2').trim() || '#00d4aa';
  var accent3 = root.getPropertyValue('--accent3').trim() || '#fd79a8';
  var warn = root.getPropertyValue('--warn').trim() || '#ffb347';
  var ink = root.getPropertyValue('--ink').trim() || '#e8eaed';
  var muted = root.getPropertyValue('--muted').trim() || '#8b8f9e';
  var bg2 = root.getPropertyValue('--bg2').trim() || '#15171f';
  var bg3 = root.getPropertyValue('--bg3').trim() || '#1e2130';
  var rule = root.getPropertyValue('--rule').trim() || '#2a2d3d';
  var feishu = root.getPropertyValue('--feishu').trim() || '#3370ff';
  var dingtalk = root.getPropertyValue('--dingtalk').trim() || '#1677ff';
  var wecom = root.getPropertyValue('--wecom').trim() || '#07c160';

  /* ============================================================
   * Chart 1 — Radar: Three platforms capability comparison
   * ============================================================ */
  function initRadar() {
    var el = document.getElementById('chart-radar');
    if (!el || typeof echarts === 'undefined') return;

    var chart = echarts.init(el, null, { renderer: 'canvas' });

    chart.setOption({
      tooltip: {
        trigger: 'item',
        backgroundColor: bg3,
        borderColor: rule,
        textStyle: { color: ink, fontSize: 12 }
      },
      legend: {
        data: ['飞书', '钉钉', '企业微信'],
        textStyle: { color: muted, fontSize: 12 },
        top: 5,
        itemGap: 20
      },
      radar: {
        center: ['50%', '55%'],
        radius: '65%',
        indicator: [
          { name: '机器人消息', max: 10 },
          { name: '文档嵌入', max: 10 },
          { name: '知识库 API', max: 10 },
          { name: '工作台应用', max: 10 },
          { name: '免登授权', max: 10 },
          { name: '卡片交互', max: 10 },
          { name: 'API 文档质量', max: 10 },
          { name: '调用额度', max: 10 }
        ],
        axisName: {
          color: ink,
          fontSize: 11
        },
        splitLine: { lineStyle: { color: rule, opacity: 0.5 } },
        splitArea: { areaStyle: { color: [bg2, 'transparent'] } },
        axisLine: { lineStyle: { color: rule } }
      },
      series: [{
        type: 'radar',
        data: [
          {
            value: [9, 9, 9, 9, 9, 10, 10, 8],
            name: '飞书',
            lineStyle: { color: feishu, width: 2 },
            itemStyle: { color: feishu },
            areaStyle: { color: feishu, opacity: 0.12 }
          },
          {
            value: [8, 7, 8, 8, 8, 8, 7, 6],
            name: '钉钉',
            lineStyle: { color: dingtalk, width: 2 },
            itemStyle: { color: dingtalk },
            areaStyle: { color: dingtalk, opacity: 0.12 }
          },
          {
            value: [7, 7, 7, 7, 8, 6, 6, 7],
            name: '企业微信',
            lineStyle: { color: wecom, width: 2 },
            itemStyle: { color: wecom },
            areaStyle: { color: wecom, opacity: 0.12 }
          }
        ]
      }],
      animation: false
    });

    window.addEventListener('resize', function () { chart.resize(); });
  }

  /* ============================================================
   * Chart 2 — Gantt: 5-phase ecosystem integration timeline
   * ============================================================ */
  function initGantt() {
    var el = document.getElementById('chart-gantt');
    if (!el || typeof echarts === 'undefined') return;

    var phases = [
      { name: 'E1 基础通知接入',   start: 0,  end: 8,  color: accent2 },
      { name: 'E2 文档深度嵌入',   start: 8,  end: 20, color: accent },
      { name: 'E3 原生应用接入',   start: 20, end: 32, color: accent3 },
      { name: 'E4 支付与合规',     start: 32, end: 40, color: warn },
      { name: 'E5 企业级深度集成', start: 40, end: 56, color: '#a29bfe' }
    ];

    var categories = phases.map(function (p) { return p.name; });

    var data = phases.map(function (p, i) {
      return {
        name: p.name,
        value: [i, p.start, p.end],
        itemStyle: { color: p.color }
      };
    });

    var chart = echarts.init(el, null, { renderer: 'canvas' });

    chart.setOption({
      tooltip: {
        trigger: 'item',
        backgroundColor: bg3,
        borderColor: rule,
        textStyle: { color: ink, fontSize: 12 },
        formatter: function (params) {
          var d = params.value;
          var startWk = d[1];
          var endWk = d[2];
          var startMonth = Math.floor(startWk / 4) + 1;
          var endMonth = Math.ceil(endWk / 4);
          return params.name + '<br/>'
            + '第 ' + (startWk + 1) + '-' + endWk + ' 周<br/>'
            + '约第 ' + startMonth + '-' + endMonth + ' 月';
        }
      },
      grid: {
        left: 150,
        right: 40,
        top: 30,
        bottom: 50
      },
      xAxis: {
        type: 'value',
        name: '周',
        nameLocation: 'middle',
        nameGap: 30,
        nameTextStyle: { color: muted, fontSize: 12 },
        min: 0,
        max: 56,
        interval: 4,
        axisLabel: {
          color: muted,
          fontSize: 11,
          formatter: function (val) {
            return val === 0 ? '开始' : '第' + val + '周';
          }
        },
        axisLine: { lineStyle: { color: rule } },
        splitLine: { lineStyle: { color: rule, type: 'dashed', opacity: 0.4 } }
      },
      yAxis: {
        type: 'category',
        data: categories,
        inverse: true,
        axisLabel: {
          color: ink,
          fontSize: 12,
          fontWeight: 600
        },
        axisLine: { lineStyle: { color: rule } },
        axisTick: { show: false }
      },
      series: [{
        type: 'custom',
        renderItem: function (params, api) {
          var catIdx = api.value(0);
          var start = api.coord([api.value(1), catIdx]);
          var end = api.coord([api.value(2), catIdx]);
          var height = api.size([0, 1])[1] * 0.5;

          return {
            type: 'rect',
            shape: {
              x: start[0],
              y: start[1] - height / 2,
              width: end[0] - start[0],
              height: height,
              r: 4
            },
            style: api.style({
              fill: api.visual('color'),
              opacity: 0.85
            })
          };
        },
        encode: { x: [1, 2], y: 0 },
        data: data,
        label: {
          show: true,
          position: 'insideRight',
          color: '#fff',
          fontSize: 11,
          fontWeight: 600,
          formatter: function (params) {
            var d = params.value;
            var dur = d[2] - d[1];
            return dur + '周';
          }
        }
      }],
      animation: false
    });

    window.addEventListener('resize', function () { chart.resize(); });
  }

  /* ============================================================
   * Chart 3 — Stacked Bar: Platform count & integration depth
   * ============================================================ */
  function initDepth() {
    var el = document.getElementById('chart-depth');
    if (!el || typeof echarts === 'undefined') return;

    var phases = ['E1 通知', 'E2 嵌入', 'E3 应用', 'E4 支付', 'E5 企业'];

    var chart = echarts.init(el, null, { renderer: 'canvas' });

    chart.setOption({
      tooltip: {
        trigger: 'axis',
        backgroundColor: bg3,
        borderColor: rule,
        textStyle: { color: ink, fontSize: 12 },
        axisPointer: { type: 'shadow' },
        formatter: function (params) {
          var str = params[0].axisValue + '<br/>';
          params.forEach(function (p) {
            str += p.marker + ' ' + p.seriesName + ': ' + (p.value > 0 ? '已接入' : '—') + '<br/>';
          });
          return str;
        }
      },
      legend: {
        data: ['飞书', '钉钉', '企业微信', '微信支付', '支付宝'],
        textStyle: { color: muted, fontSize: 11 },
        top: 5,
        itemGap: 15
      },
      grid: {
        left: 50,
        right: 40,
        top: 50,
        bottom: 40
      },
      xAxis: {
        type: 'category',
        data: phases,
        axisLabel: {
          color: ink,
          fontSize: 12,
          fontWeight: 600
        },
        axisLine: { lineStyle: { color: rule } },
        axisTick: { show: false }
      },
      yAxis: {
        type: 'value',
        name: '接入状态',
        nameTextStyle: { color: muted, fontSize: 11 },
        min: 0,
        max: 3,
        interval: 1,
        axisLabel: {
          color: muted,
          fontSize: 11,
          formatter: function (val) {
            var labels = { 0: '未接入', 1: '基础', 2: '深度', 3: '完整' };
            return labels[val] || '';
          }
        },
        axisLine: { show: false },
        splitLine: { lineStyle: { color: rule, type: 'dashed', opacity: 0.4 } }
      },
      series: [
        {
          name: '飞书',
          type: 'bar',
          barWidth: 18,
          data: [1, 2, 3, 0, 3],
          itemStyle: { color: feishu, borderRadius: [3, 3, 0, 0] }
        },
        {
          name: '钉钉',
          type: 'bar',
          barWidth: 18,
          data: [1, 2, 3, 0, 3],
          itemStyle: { color: dingtalk, borderRadius: [3, 3, 0, 0] }
        },
        {
          name: '企业微信',
          type: 'bar',
          barWidth: 18,
          data: [1, 2, 3, 0, 3],
          itemStyle: { color: wecom, borderRadius: [3, 3, 0, 0] }
        },
        {
          name: '微信支付',
          type: 'bar',
          barWidth: 18,
          data: [0, 0, 0, 3, 3],
          itemStyle: { color: accent, borderRadius: [3, 3, 0, 0] }
        },
        {
          name: '支付宝',
          type: 'bar',
          barWidth: 18,
          data: [0, 0, 0, 3, 3],
          itemStyle: { color: accent3, borderRadius: [3, 3, 0, 0] }
        }
      ],
      animation: false
    });

    window.addEventListener('resize', function () { chart.resize(); });
  }

  // Initialize on DOM ready
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', function () {
      initRadar();
      initGantt();
      initDepth();
    });
  } else {
    initRadar();
    initGantt();
    initDepth();
  }
})();
