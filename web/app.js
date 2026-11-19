"use strict";

const $ = (selector) => document.querySelector(selector);
const $$ = (selector) => [...document.querySelectorAll(selector)];
const icon = (name) =>
  `<svg aria-hidden="true"><use href="#i-${name}"></use></svg>`;
const escapeHtml = (value) =>
  String(value).replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ],
  );
const ui = {
  project: null,
  selectedClip: 1,
  selectedAsset: 1,
  frame: 50,
  zoom: 82,
  playing: false,
  muted: false,
  snap: true,
  playStart: 0,
  playFrame: 0,
  videoUrl: null,
  audioUrl: null,
  videoTarget: 0,
  audioTarget: 0,
  toastTimer: null,
  warningShown: false,
};
const video = $("#programVideo");
const audio = $("#programAudio");

function timecode(frame) {
  const fps = ui.project?.fps || 25;
  frame = Math.max(0, Math.round(frame));
  const hours = Math.floor(frame / (fps * 3600));
  const minutes = Math.floor(frame / (fps * 60)) % 60;
  const seconds = Math.floor(frame / fps) % 60;
  const frames = frame % fps;
  return [hours, minutes, seconds, frames]
    .map((n) => String(n).padStart(2, "0"))
    .join(":");
}
function shortDuration(frame) {
  const secs = Math.floor(frame / (ui.project?.fps || 25));
  return `${String(Math.floor(secs / 60)).padStart(2, "0")}:${String(secs % 60).padStart(2, "0")}`;
}
function toast(message, error = false) {
  const el = $("#toast");
  el.textContent = message;
  el.classList.toggle("error", error);
  el.classList.add("visible");
  clearTimeout(ui.toastTimer);
  ui.toastTimer = setTimeout(() => el.classList.remove("visible"), 3800);
}
function asset(id) {
  return ui.project?.assets.find((item) => item.id === id);
}
function clip(id) {
  return ui.project?.clips.find((item) => item.id === id);
}
function clipAt(track, frame) {
  return ui.project?.clips.find(
    (c) =>
      c.track === track && c.start <= frame && frame < c.start + c.duration,
  );
}
async function responseJson(response) {
  const data = await response
    .json()
    .catch(() => ({ error: "Сервер вернул некорректный ответ" }));
  if (!response.ok)
    throw new Error(data.error || "Не удалось выполнить операцию");
  return data;
}
async function loadProject() {
  const response = await fetch("/api/project", { cache: "no-store" });
  ui.project = await responseJson(response);
  if (ui.selectedClip && !clip(ui.selectedClip))
    ui.selectedClip = ui.project.clips[0]?.id || null;
  if (ui.selectedAsset && !asset(ui.selectedAsset))
    ui.selectedAsset = ui.project.assets[0]?.id || null;
  ui.frame = Math.min(ui.frame, ui.project.length);
  render();
  syncMedia();
  if (ui.project.startup_warning && !ui.warningShown) {
    ui.warningShown = true;
    toast(ui.project.startup_warning, true);
  }
}
async function command(op, values = {}) {
  pause();
  try {
    const body = new URLSearchParams({ op, ...values });
    const result = await responseJson(
      await fetch("/api/command", { method: "POST", body }),
    );
    if (result.created) ui.selectedClip = result.created;
    if (op === "delete") ui.selectedClip = null;
    await loadProject();
    return result;
  } catch (error) {
    toast(error.message, true);
    renderTimeline();
    return null;
  }
}

function render() {
  const p = ui.project;
  if (!p) return;
  $("#projectName").textContent = p.name;
  $("#monitorSequence").textContent = p.name;
  $("#timelineName").textContent = p.name;
  $("#assetCount").textContent = String(p.assets.length).padStart(2, "0");
  $("#mediaTotal").textContent =
    `${p.assets.length} ${plural(p.assets.length, "файл", "файла", "файлов")}`;
  $("#clipCount").textContent =
    `${p.clips.length} ${plural(p.clips.length, "КЛИП", "КЛИПА", "КЛИПОВ")}`;
  $("#durationTime").textContent = timecode(p.length);
  $("#engineStatus").textContent =
    p.ffmpeg && p.ffprobe
      ? "Rust-ядро и FFmpeg готовы"
      : "Rust-ядро готово · FFmpeg / ffprobe не найден";
  $("#saveStatus").innerHTML =
    `<span></span> ${p.saved ? "Сохранено на сервере" : "Не сохранено"}`;
  $("#saveStatus").classList.toggle("saved", p.saved);
  $("#footerSave").textContent = p.saved
    ? "СОХРАНЕНО НА СЕРВЕРЕ"
    : "ПРОЕКТ НЕ СОХРАНЁН";
  $("#exportButton").title = p.ffmpeg
    ? "Экспорт в H.264 MP4"
    : "Для экспорта установите FFmpeg";
  renderMedia();
  renderInspector();
  renderTimeline();
  updateTimes();
  updateButtons();
}
function plural(number, one, few, many) {
  const n = number % 100;
  if (n >= 11 && n <= 14) return many;
  const last = n % 10;
  return last === 1 ? one : last >= 2 && last <= 4 ? few : many;
}

function renderMedia() {
  const container = $("#mediaGrid");
  const query = $("#mediaSearch").value.trim().toLocaleLowerCase();
  const assets = ui.project.assets.filter((a) =>
    a.name.toLocaleLowerCase().includes(query),
  );
  if (!assets.length) {
    container.innerHTML = `<div class="media-empty">${query ? "Ничего не найдено." : "Здесь появятся ваши файлы."}</div>`;
    return;
  }
  container.innerHTML = assets
    .map((item) => {
      const image = item.thumbnail
        ? `<img src="${item.thumbnail}" alt="" loading="lazy">`
        : icon(item.has_video ? "video" : "music");
      const details = item.has_video
        ? `${item.width} × ${item.height}`
        : "Аудиофайл";
      return `<div class="media-card ${item.id === ui.selectedAsset ? "selected" : ""}" role="button" tabindex="0" draggable="true" data-asset-id="${item.id}" aria-label="${escapeHtml(item.name)}. Дважды нажмите, чтобы добавить на таймлайн">
      <div class="thumb ${item.thumbnail ? "" : "audio-thumb"}">${image}<span class="thumb-kind">${icon(item.has_video ? "video" : "music")}</span><span class="thumb-duration">${shortDuration(item.duration)}</span><button class="add-asset" data-add="${item.id}" aria-label="Добавить ${escapeHtml(item.name)} на таймлайн" title="Добавить на таймлайн">${icon("plus")}</button></div>
      <div class="asset-name" title="${escapeHtml(item.name)}">${escapeHtml(item.name)}</div><div class="asset-sub">${details}</div></div>`;
    })
    .join("");
  container.querySelectorAll(".media-card").forEach((card) => {
    const id = Number(card.dataset.assetId);
    card.addEventListener("click", (event) => {
      const add = event.target.closest("[data-add]");
      if (add) {
        event.stopPropagation();
        addAsset(id);
        return;
      }
      ui.selectedAsset = id;
      container.querySelectorAll(".media-card").forEach((other) => {
        other.classList.toggle(
          "selected",
          Number(other.dataset.assetId) === id,
        );
      });
    });
    card.addEventListener("dblclick", (event) => {
      if (!event.target.closest("[data-add]")) addAsset(id);
    });
    card.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        addAsset(id);
      }
    });
    card.addEventListener("dragstart", (event) => {
      event.dataTransfer.effectAllowed = "copy";
      event.dataTransfer.setData("application/x-rustcut-asset", String(id));
      event.dataTransfer.setData("text/plain", String(id));
    });
  });
}
async function addAsset(id, track, start = null) {
  const item = asset(id);
  if (!item) return;
  const target = track || (item.has_video ? "V1" : "A1");
  const values = { asset_id: id, track: target };
  if (start !== null) {
    values.start = snapFrame(
      Math.max(0, start),
      { id: -1, track: target, duration: item.duration },
      "move",
    );
  }
  const result = await command("add", values);
  if (result) {
    ui.selectedAsset = id;
    renderMedia();
    toast(
      start === null
        ? "Клип добавлен в конец дорожки"
        : "Клип добавлен на таймлайн",
    );
  }
}

function renderInspector() {
  const selected = clip(ui.selectedClip);
  const item = selected && asset(selected.asset_id);
  const card = $("#selectedCard");
  const props = $("#properties");
  $(".live-pill").classList.toggle("hidden", !selected || !item);
  if (!selected || !item) {
    card.innerHTML =
      '<span class="selected-empty">Выберите клип на таймлайне, чтобы увидеть его свойства.</span>';
    props.innerHTML =
      "<dt>Дорожка</dt><dd>—</dd><dt>Начало</dt><dd>—</dd><dt>Вход / выход</dt><dd>—</dd><dt>Длительность</dt><dd>—</dd>";
    return;
  }
  const image = item.thumbnail
    ? `<img class="selected-image" src="${item.thumbnail}" alt="">`
    : `<span class="selected-audio">${icon("music")}</span>`;
  card.innerHTML = `${image}<div class="selected-info"><strong title="${escapeHtml(item.name)}">${escapeHtml(item.name)}</strong><small>${selected.track === "V1" ? "Видеоклип" : "Аудиоклип"} · ${selected.track}</small></div>`;
  props.innerHTML = `<dt>Дорожка</dt><dd>${selected.track}</dd><dt>Начало</dt><dd>${timecode(selected.start)}</dd><dt>Вход</dt><dd>${timecode(selected.source_in)}</dd><dt>Выход</dt><dd>${timecode(selected.source_in + selected.duration)}</dd><dt>Длительность</dt><dd>${timecode(selected.duration)}</dd>`;
}

function waveform(item, c, width) {
  const peaks = item.waveform || [];
  if (!peaks.length)
    return '<svg class="waveform" viewBox="0 0 100 30" preserveAspectRatio="none"><path d="M0 15H100" stroke="currentColor" stroke-width="1"/></svg>';
  const bars = Math.max(8, Math.min(230, Math.floor(width / 3)));
  let path = "";
  for (let i = 0; i < bars; i++) {
    const sourceFrame = c.source_in + (c.duration * i) / bars;
    const index = Math.min(
      peaks.length - 1,
      Math.floor((sourceFrame / item.duration) * peaks.length),
    );
    const h = Math.max(1, ((peaks[index] || 0) / 100) * 13);
    path += `M${i * 3 + 1} ${(15 - h).toFixed(1)}v${(h * 2).toFixed(1)} `;
  }
  return `<svg class="waveform" viewBox="0 0 ${bars * 3} 30" preserveAspectRatio="none"><path d="${path}" stroke="currentColor" stroke-width="1.4"/></svg>`;
}
function renderTimeline() {
  if (!ui.project) return;
  const fps = ui.project.fps;
  const scroll = $("#timelineScroll");
  const canvas = $("#timelineCanvas");
  const seconds = Math.max(
    16,
    ui.project.length / fps + 3,
    scroll.clientWidth / ui.zoom,
  );
  const width = Math.ceil(seconds * ui.zoom);
  canvas.style.width = `${width}px`;
  canvas.style.setProperty("--grid-size", `${ui.zoom}px`);
  const ruler = $("#ruler");
  let ticks = "";
  for (let half = 0; half <= Math.floor(seconds * 2); half++) {
    const major = half % 4 === 0;
    const at = half / 2;
    ticks += `<div class="ruler-tick ${major ? "major" : ""}" style="left:${at * ui.zoom}px">${major ? `<span>${timecode(at * fps)}</span>` : ""}</div>`;
  }
  ruler.innerHTML = ticks;
  for (const track of ["V1", "A1"]) {
    const lane = track === "V1" ? $("#videoLane") : $("#audioLane");
    lane.innerHTML = "";
    ui.project.clips
      .filter((c) => c.track === track)
      .forEach((c) => {
        const item = asset(c.asset_id);
        if (!item) return;
        const el = document.createElement("div");
        const left = (c.start / fps) * ui.zoom;
        const clipWidth = Math.max(4, (c.duration / fps) * ui.zoom);
        el.className = `timeline-clip ${track === "V1" ? "video-clip" : "audio-clip"} ${c.id === ui.selectedClip ? "selected" : ""}`;
        el.style.left = `${left}px`;
        el.style.width = `${clipWidth}px`;
        el.dataset.clipId = c.id;
        el.tabIndex = 0;
        el.setAttribute("role", "button");
        el.setAttribute(
          "aria-label",
          `${item.name}, ${track}, ${timecode(c.start)}, длительность ${timecode(c.duration)}`,
        );
        el.title = `${item.name}\n${timecode(c.start)} — ${timecode(c.start + c.duration)}\nПеретащите, чтобы переместить; потяните за край, чтобы обрезать`;
        const visual =
          track === "V1"
            ? `<div class="filmstrip" ${item.thumbnail ? `style="background-image:url('${item.thumbnail}')"` : ""}></div>`
            : waveform(item, c, clipWidth);
        el.innerHTML = `${visual}<div class="clip-head">${icon(track === "V1" ? "video" : "music")}<span>${escapeHtml(item.name)}</span></div><div class="trim-handle left" data-edge="left"></div><div class="trim-handle right" data-edge="right"></div>`;
        el.addEventListener("pointerdown", (event) =>
          startClipDrag(event, c, el),
        );
        el.addEventListener("keydown", (event) => {
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            selectClip(c.id);
          }
        });
        lane.append(el);
      });
  }
  $("#playhead").style.left = `${(ui.frame / fps) * ui.zoom}px`;
  $("#zoomValue").textContent = `${Math.round((ui.zoom / 82) * 100)}%`;
}
function selectClip(id) {
  ui.selectedClip = id;
  const c = clip(id);
  if (c) ui.selectedAsset = c.asset_id;
  $$(".timeline-clip").forEach((el) =>
    el.classList.toggle("selected", Number(el.dataset.clipId) === id),
  );
  renderMedia();
  renderInspector();
  updateButtons();
}
function snapFrame(value, movingClip, mode) {
  if (!ui.snap) return value;
  const threshold = Math.max(1, Math.round((8 / ui.zoom) * ui.project.fps));
  const edges = [0, ui.frame];
  ui.project.clips
    .filter((c) => c.track === movingClip.track && c.id !== movingClip.id)
    .forEach((c) => edges.push(c.start, c.start + c.duration));
  let result = value,
    best = threshold + 1;
  edges.forEach((edge) => {
    const candidates =
      mode === "move" ? [edge, edge - movingClip.duration] : [edge];
    candidates.forEach((candidate) => {
      const distance = Math.abs(candidate - value);
      if (candidate >= 0 && distance < best && distance <= threshold) {
        best = distance;
        result = candidate;
      }
    });
  });
  return result;
}
function startClipDrag(event, original, el) {
  if (event.button !== 0 || !ui.project) return;
  event.preventDefault();
  event.stopPropagation();
  pause();
  selectClip(original.id);
  const mode = event.target.dataset.edge || "move";
  const originX = event.clientX;
  const start = original.start,
    end = original.start + original.duration;
  let value = mode === "right" ? end : start;
  let moved = false;
  el.setPointerCapture(event.pointerId);
  el.classList.add("dragging");
  const move = (e) => {
    const dx = e.clientX - originX;
    if (Math.abs(dx) > 3) moved = true;
    const delta = Math.round((dx / ui.zoom) * ui.project.fps);
    if (mode === "move") {
      value = snapFrame(Math.max(0, start + delta), original, mode);
      el.style.left = `${(value / ui.project.fps) * ui.zoom}px`;
    } else if (mode === "left") {
      value = snapFrame(
        Math.max(
          0,
          start - original.source_in,
          Math.min(end - 1, start + delta),
        ),
        original,
        mode,
      );
      value = Math.max(0, start - original.source_in, Math.min(end - 1, value));
      el.style.left = `${(value / ui.project.fps) * ui.zoom}px`;
      el.style.width = `${((end - value) / ui.project.fps) * ui.zoom}px`;
    } else {
      const max =
        start + asset(original.asset_id).duration - original.source_in;
      value = snapFrame(
        Math.max(start + 1, Math.min(max, end + delta)),
        original,
        mode,
      );
      value = Math.max(start + 1, Math.min(max, value));
      el.style.width = `${((value - start) / ui.project.fps) * ui.zoom}px`;
    }
  };
  const finish = (e) => {
    el.removeEventListener("pointermove", move);
    el.removeEventListener("pointerup", finish);
    el.removeEventListener("pointercancel", cancel);
    el.classList.remove("dragging");
    if (!moved || (mode === "right" ? value === end : value === start)) {
      renderTimeline();
      return;
    }
    if (mode === "move")
      command("move", { clip_id: original.id, start: value });
    else if (mode === "left")
      command("trim_left", { clip_id: original.id, start: value });
    else command("trim_right", { clip_id: original.id, end: value });
  };
  const cancel = () => {
    el.removeEventListener("pointermove", move);
    el.removeEventListener("pointerup", finish);
    renderTimeline();
  };
  el.addEventListener("pointermove", move);
  el.addEventListener("pointerup", finish);
  el.addEventListener("pointercancel", cancel);
}
function updateTimes() {
  $("#monitorTime").textContent = timecode(ui.frame);
  $("#timelineTime").textContent = timecode(ui.frame);
  if (ui.project)
    $("#playhead").style.left = `${(ui.frame / ui.project.fps) * ui.zoom}px`;
  updateButtons();
}
function updateButtons() {
  const p = ui.project;
  if (!p) return;
  const selected = clip(ui.selectedClip);
  $("#undoButton").disabled = !p.can_undo;
  $("#redoButton").disabled = !p.can_redo;
  $("#deleteButton").disabled = !selected;
  $("#splitButton").disabled =
    !selected ||
    ui.frame <= selected.start ||
    ui.frame >= selected.start + selected.duration;
  const label = ui.playing ? "Пауза" : "Воспроизвести";
  if ($("#playButton").getAttribute("aria-label") !== label) {
    $("#playButton").setAttribute("aria-label", label);
    $("#playButton").innerHTML = icon(ui.playing ? "pause" : "play");
  }
}
function seek(frame) {
  if (!ui.project) return;
  ui.frame = Math.max(0, Math.min(ui.project.length, Math.round(frame)));
  if (ui.playing) {
    ui.playFrame = ui.frame;
    ui.playStart = performance.now();
  }
  updateTimes();
  syncMedia();
}
function setSource(element, key, source, desired) {
  const urlKey = `${key}Url`;
  const targetKey = `${key}Target`;
  ui[targetKey] = desired;
  if (ui[urlKey] !== source) {
    element.pause();
    ui[urlKey] = source;
    element.src = source;
    element.load();
  }
  if (element.readyState >= 1 && Number.isFinite(element.duration)) {
    const target = Math.min(desired, Math.max(0, element.duration - 0.001));
    if (Math.abs(element.currentTime - target) > (ui.playing ? 0.2 : 0.025)) {
      try {
        element.currentTime = target;
      } catch (_) {
        /* metadata may still be loading */
      }
    }
  }
  element.muted = ui.muted;
  if (ui.playing && element.paused) {
    const promise = element.play();
    if (promise)
      promise.catch((error) => {
        if (error.name === "NotSupportedError")
          toast("Браузер не поддерживает кодек этого файла", true);
      });
  } else if (!ui.playing && !element.paused) element.pause();
}
function syncMedia() {
  if (!ui.project) return;
  const displayFrame = ui.project.length
    ? Math.min(ui.frame, ui.project.length - 1)
    : 0;
  const currentVideo = clipAt("V1", displayFrame);
  const currentAudio = clipAt("A1", displayFrame);
  if (currentVideo) {
    const item = asset(currentVideo.asset_id);
    $("#monitorEmpty").classList.remove("visible");
    $("#frameTag").classList.remove("hidden");
    video.poster = item.thumbnail || "";
    setSource(
      video,
      "video",
      item.url,
      (currentVideo.source_in + displayFrame - currentVideo.start) /
        ui.project.fps,
    );
  } else {
    video.pause();
    $("#monitorEmpty").classList.add("visible");
    $("#frameTag").classList.add("hidden");
  }
  if (currentAudio) {
    const item = asset(currentAudio.asset_id);
    setSource(
      audio,
      "audio",
      item.url,
      (currentAudio.source_in + displayFrame - currentAudio.start) /
        ui.project.fps,
    );
  } else audio.pause();
}
video.addEventListener("loadedmetadata", () => {
  if (Number.isFinite(video.duration))
    video.currentTime = Math.min(
      ui.videoTarget,
      Math.max(0, video.duration - 0.001),
    );
});
audio.addEventListener("loadedmetadata", () => {
  if (Number.isFinite(audio.duration))
    audio.currentTime = Math.min(
      ui.audioTarget,
      Math.max(0, audio.duration - 0.001),
    );
});
function pause() {
  if (!ui.playing) return;
  ui.playing = false;
  video.pause();
  audio.pause();
  updateButtons();
}
function togglePlay() {
  if (!ui.project || !ui.project.length) {
    toast("Добавьте клип на таймлайн");
    return;
  }
  if (ui.playing) {
    pause();
    return;
  }
  if (ui.frame >= ui.project.length) ui.frame = 0;
  ui.playing = true;
  ui.playFrame = ui.frame;
  ui.playStart = performance.now();
  updateButtons();
  syncMedia();
  requestAnimationFrame(tick);
}
function tick() {
  if (!ui.playing) return;
  // An animation-frame timestamp can be stale after a stalled main thread.
  const frame =
    ui.playFrame +
    Math.floor(((performance.now() - ui.playStart) / 1000) * ui.project.fps);
  if (frame >= ui.project.length) {
    ui.frame = ui.project.length;
    pause();
    updateTimes();
    syncMedia();
    return;
  }
  if (frame !== ui.frame) {
    ui.frame = frame;
    updateTimes();
    syncMedia();
  }
  requestAnimationFrame(tick);
}

$("#timelineCanvas").addEventListener("pointerdown", (event) => {
  if (event.button !== 0 || event.target.closest(".timeline-clip")) return;
  pause();
  const canvas = $("#timelineCanvas");
  const fromEvent = (e) =>
    seek(
      ((e.clientX - canvas.getBoundingClientRect().left) / ui.zoom) *
        ui.project.fps,
    );
  fromEvent(event);
  canvas.setPointerCapture(event.pointerId);
  const move = (e) => fromEvent(e);
  const finish = () => {
    canvas.removeEventListener("pointermove", move);
    canvas.removeEventListener("pointerup", finish);
    canvas.removeEventListener("pointercancel", finish);
  };
  canvas.addEventListener("pointermove", move);
  canvas.addEventListener("pointerup", finish);
  canvas.addEventListener("pointercancel", finish);
});
$("#timelineScroll").addEventListener("dragover", (event) => {
  event.preventDefault();
  event.dataTransfer.dropEffect = "copy";
});
$("#timelineScroll").addEventListener("drop", (event) => {
  event.preventDefault();
  const id = Number(
    event.dataTransfer.getData("application/x-rustcut-asset") ||
      event.dataTransfer.getData("text/plain"),
  );
  const lane = event.target.closest("[data-track]");
  const track = lane?.dataset.track;
  if (!id) return;
  if (track && track !== "V1" && track !== "A1") {
    toast("Пока доступны только дорожки V1 и A1", true);
  } else if (track) {
    const rect = $("#timelineCanvas").getBoundingClientRect();
    const start = Math.max(
      0,
      Math.round(((event.clientX - rect.left) / ui.zoom) * ui.project.fps),
    );
    addAsset(id, track, start);
  } else addAsset(id);
});
$("#timelineScroll").addEventListener("scroll", () => {
  const top = $("#timelineScroll").scrollTop;
  $$(".track-labels > *").forEach((label) => {
    label.style.transform = `translateY(${-top}px)`;
  });
});
$("#playButton").addEventListener("click", togglePlay);
$("#goStart").addEventListener("click", () => {
  pause();
  seek(0);
});
$("#goEnd").addEventListener("click", () => {
  pause();
  seek(ui.project?.length || 0);
});
$("#stepBack").addEventListener("click", () => {
  pause();
  seek(ui.frame - 1);
});
$("#stepForward").addEventListener("click", () => {
  pause();
  seek(ui.frame + 1);
});
$("#muteButton").addEventListener("click", () => {
  ui.muted = !ui.muted;
  video.muted = ui.muted;
  audio.muted = ui.muted;
  $("#muteButton").innerHTML = icon(ui.muted ? "muted" : "volume");
  $("#muteButton").title = ui.muted ? "Включить звук" : "Выключить звук";
  $("#muteButton").setAttribute("aria-label", $("#muteButton").title);
});
$("#splitButton").addEventListener("click", () => {
  if (ui.selectedClip)
    command("split", { clip_id: ui.selectedClip, at: ui.frame });
});
$("#deleteButton").addEventListener("click", () => {
  if (ui.selectedClip) command("delete", { clip_id: ui.selectedClip });
});
$("#undoButton").addEventListener("click", () => command("undo"));
$("#redoButton").addEventListener("click", () => command("redo"));
$("#snapButton").addEventListener("click", () => {
  ui.snap = !ui.snap;
  $("#snapButton").classList.toggle("snap-active", ui.snap);
  $("#snapButton").setAttribute("aria-pressed", String(ui.snap));
  toast(ui.snap ? "Привязка включена" : "Привязка выключена");
});
$("#selectButton").addEventListener("click", () =>
  toast("Перетащите клип или его край"),
);
function setZoom(value) {
  ui.zoom = Math.max(35, Math.min(160, Number(value)));
  $("#zoomSlider").value = ui.zoom;
  renderTimeline();
}
$("#zoomSlider").addEventListener("input", (event) =>
  setZoom(event.target.value),
);
$("#zoomIn").addEventListener("click", () => setZoom(ui.zoom + 10));
$("#zoomOut").addEventListener("click", () => setZoom(ui.zoom - 10));
$("#searchToggle").addEventListener("click", () => {
  $("#searchWrap").classList.toggle("hidden");
  if (!$("#searchWrap").classList.contains("hidden")) $("#mediaSearch").focus();
  else {
    $("#mediaSearch").value = "";
    renderMedia();
  }
});
$("#mediaSearch").addEventListener("input", renderMedia);
$("#helpButton").addEventListener("click", () => $("#helpDialog").showModal());
$("#closeHelp").addEventListener("click", () => $("#helpDialog").close());
$("#helpDialog").addEventListener("click", (event) => {
  if (event.target === $("#helpDialog")) $("#helpDialog").close();
});

async function importFiles(files) {
  if (!files.length) return;
  if (!ui.project.ffprobe) {
    toast("Для импорта установите ffprobe", true);
    return;
  }
  for (const file of files) {
    if (file.size > 100 * 1024 * 1024) {
      toast(`${file.name}: максимум 100 МБ`, true);
      continue;
    }
    toast(`Импорт: ${file.name}…`);
    try {
      const result = await responseJson(
        await fetch(`/api/import?name=${encodeURIComponent(file.name)}`, {
          method: "POST",
          headers: { "Content-Type": "application/octet-stream" },
          body: file,
        }),
      );
      ui.selectedAsset = result.asset_id;
      await loadProject();
      toast(`Импортировано: ${file.name}`);
    } catch (error) {
      toast(`${file.name}: ${error.message}`, true);
    }
  }
}
$("#importButton").addEventListener("click", () => $("#fileInput").click());
$("#importIcon").addEventListener("click", () => $("#fileInput").click());
$("#fileInput").addEventListener("change", (event) => {
  importFiles([...event.target.files]);
  event.target.value = "";
});
const mediaPanel = $(".media-panel");
mediaPanel.addEventListener("dragover", (event) => {
  if ([...event.dataTransfer.types].includes("Files")) {
    event.preventDefault();
    mediaPanel.classList.add("drag-over");
  }
});
mediaPanel.addEventListener("dragleave", (event) => {
  if (!mediaPanel.contains(event.relatedTarget))
    mediaPanel.classList.remove("drag-over");
});
mediaPanel.addEventListener("drop", (event) => {
  if (event.dataTransfer.files.length) {
    event.preventDefault();
    mediaPanel.classList.remove("drag-over");
    importFiles([...event.dataTransfer.files]);
  }
});
async function saveProject() {
  if (!ui.project) return;
  const button = $("#saveButton");
  button.disabled = true;
  try {
    const result = await responseJson(
      await fetch("/api/save", { method: "POST", body: "" }),
    );
    await loadProject();
    toast(
      result.current
        ? "Проект сохранён на сервере"
        : "Сохранена предыдущая версия; есть новые изменения",
    );
  } catch (error) {
    toast(error.message, true);
  } finally {
    button.disabled = false;
  }
}
$("#saveButton").addEventListener("click", saveProject);
$("#exportButton").addEventListener("click", async () => {
  if (!ui.project?.ffmpeg) {
    toast("Для экспорта установите FFmpeg с libx264", true);
    return;
  }
  pause();
  const button = $("#exportButton");
  button.disabled = true;
  button.innerHTML = `${icon("download")}<span>Рендер…</span>`;
  toast("Рендерим последовательность в H.264 MP4…");
  try {
    const result = await responseJson(
      await fetch("/api/export", { method: "POST", body: "" }),
    );
    const link = document.createElement("a");
    link.href = result.url;
    link.download = "rustcut-export.mp4";
    document.body.append(link);
    link.click();
    link.remove();
    toast("Экспорт готов. Загрузка началась.");
  } catch (error) {
    toast(error.message, true);
  } finally {
    button.disabled = false;
    button.innerHTML = `${icon("external")}<span>Экспорт</span>`;
  }
});
window.addEventListener("keydown", (event) => {
  if (event.target.matches("input, textarea") || $("#helpDialog").open) return;
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
    event.preventDefault();
    saveProject();
    return;
  }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "z") {
    event.preventDefault();
    if (event.shiftKey && ui.project?.can_redo) command("redo");
    else if (!event.shiftKey && ui.project?.can_undo) command("undo");
    return;
  }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "y") {
    event.preventDefault();
    if (ui.project?.can_redo) command("redo");
    return;
  }
  if (event.code === "Space") {
    event.preventDefault();
    togglePlay();
  } else if (event.key === "ArrowLeft") {
    event.preventDefault();
    pause();
    seek(ui.frame - 1);
  } else if (event.key === "ArrowRight") {
    event.preventDefault();
    pause();
    seek(ui.frame + 1);
  } else if (event.key.toLowerCase() === "c" && !$("#splitButton").disabled) {
    event.preventDefault();
    $("#splitButton").click();
  } else if (
    (event.key === "Delete" || event.key === "Backspace") &&
    ui.selectedClip
  ) {
    event.preventDefault();
    $("#deleteButton").click();
  }
});
window.addEventListener("resize", () => {
  if (ui.project) renderTimeline();
});
loadProject().catch((error) => {
  $("#engineStatus").textContent = "Нет связи с Rust-ядром";
  toast(`Не удалось загрузить проект: ${error.message}`, true);
});
