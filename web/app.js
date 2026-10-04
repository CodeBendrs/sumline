"use strict";
const $ = (id) => document.getElementById(id);
const PRODUCT_NAME = document.querySelector(
  'meta[name="application-name"]',
).content;
const esc = (v) =>
  String(v ?? "").replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ],
  );
const state = {
  me: null,
  records: [],
  connections: null,
  view: "invoices",
  search: "",
  store: "",
  mode: "login",
  policy: null,
  selected: null,
  page: 1,
  total: 0,
  counts: {},
  refreshId: 0,
  uploading: false,
  drawerDirty: false,
  drawerRequest: 0,
  supplier: "",
  status: "",
  currency: "",
  from: "",
  to: "",
  sort: "received_newest",
  period: "all",
  suppliers: [],
  currencies: [],
  currencyOptions: [],
};
const statusLabels = {
  ready: "In spreadsheet",
  approved: "Reviewed",
  review: "Needs review",
  processing: "Processing",
  excluded: "Excluded",
  bundle: "Split attachment",
};
const date = (v) =>
  v
    ? new Intl.DateTimeFormat("en-GB", {
        year: "numeric",
        month: "short",
        day: "numeric",
        timeZone: "UTC",
      }).format(new Date(v.length === 10 ? v + "T12:00:00Z" : v))
    : "—";
const amount = (c, currency) => {
  if (c == null) return "—";
  const figure = new Intl.NumberFormat("en-CA", {
    style: "currency",
    currency: currency || "CAD",
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  }).format(Math.abs(c) / 100);
  return c < 0 ? `−${figure} credit` : figure;
};
const money = (c) => (c == null ? "" : (c / 100).toFixed(2));
const badge = (r) =>
  `<span class="badge ${esc(r.status)}"><svg class="status-icon" viewBox="0 0 20 20" aria-hidden="true"><use href="#t-${esc(r.status)}"></use></svg>${esc(statusLabels[r.status] || r.status)}</span>`;
const storeName = (id) =>
  state.me?.stores.find((s) => s.id === id)?.name || "Unassigned";
async function api(path, options = {}) {
  const headers = { ...(options.headers || {}) };
  if (options.method && options.method !== "GET") {
    headers["X-CSRF-Token"] = state.me?.csrf || "";
  }
  if (options.body && !(options.body instanceof FormData)) {
    headers["Content-Type"] = "application/json";
    options.body = JSON.stringify(options.body);
  }
  let res;
  try {
    res = await fetch(path, { ...options, headers });
  } catch {
    throw new Error("Connection lost. Please retry your last action.");
  }
  let body = {};
  try {
    body = await res.json();
  } catch {}
  if (!res.ok) {
    const e = new Error(body.error || "This request could not be completed.");
    e.status = res.status;
    throw e;
  }
  return body;
}
function toast(message) {
  $("toast").textContent = message;
  $("toast").hidden = false;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => ($("toast").hidden = true), 4500);
}
function closeAccountMenu(restoreFocus = false) {
  $("account-menu").hidden = true;
  $("account-toggle").setAttribute("aria-expanded", "false");
  if (restoreFocus) $("account-toggle").focus();
}
function closeNavigation() {
  if ($("mobile-navigation").open) $("mobile-navigation").close();
  $("open-navigation").setAttribute("aria-expanded", "false");
}
const mobileLayout = window.matchMedia("(max-width: 760px)");
function syncResponsiveNavigation() {
  closeAccountMenu();
  closeNavigation();
  if (mobileLayout.matches) {
    $("mobile-navigation").append($("sidebar"));
    $("mobile-account-slot").append($("account-controls"));
  } else {
    $("app").prepend($("sidebar"));
    $("desktop-account-slot").append($("account-controls"));
  }
}
$("account-toggle").onclick = () => {
  const opening = $("account-menu").hidden;
  $("account-menu").hidden = !opening;
  $("account-toggle").setAttribute("aria-expanded", String(opening));
  if (opening) $("logout").focus();
};
$("account-toggle").addEventListener("keydown", (e) => {
  if (["ArrowDown", "ArrowUp"].includes(e.key)) {
    e.preventDefault();
    $("account-menu").hidden = false;
    $("account-toggle").setAttribute("aria-expanded", "true");
    $("logout").focus();
  }
});
$("account-controls").addEventListener("keydown", (e) => {
  if (
    !$("account-menu").hidden &&
    ["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)
  ) {
    e.preventDefault();
    $("logout").focus();
  }
  if (e.key === "Escape" && !$("account-menu").hidden) {
    e.preventDefault();
    closeAccountMenu(true);
  }
});
$("account-controls").addEventListener("focusout", (e) => {
  if (!e.currentTarget.contains(e.relatedTarget)) closeAccountMenu();
});
document.addEventListener("click", (e) => {
  if (!$("account-controls").contains(e.target)) closeAccountMenu();
});
$("open-navigation").onclick = () => {
  closeAccountMenu();
  $("mobile-navigation").showModal();
  $("open-navigation").setAttribute("aria-expanded", "true");
  $("close-navigation").focus();
};
$("close-navigation").onclick = closeNavigation;
$("mobile-navigation").addEventListener("close", () => {
  $("open-navigation").setAttribute("aria-expanded", "false");
});
$("mobile-navigation").addEventListener("click", (e) => {
  if (e.target === e.currentTarget) closeNavigation();
});
$("signout-dialog").addEventListener("close", () => {
  if (state.me) $("account-toggle").focus();
});
mobileLayout.addEventListener("change", syncResponsiveNavigation);
syncResponsiveNavigation();

async function load() {
  try {
    state.me = await api("/api/me");
    $("auth").hidden = true;
    $("app").hidden = false;
    renderUser();
    await refresh();
    const invite = new URLSearchParams(location.search).get("invite");
    if (invite) {
      try {
        await api("/api/invitations/accept", {
          method: "POST",
          body: { token: invite },
        });
        history.replaceState(null, "", "/");
        toast("Workspace invitation accepted.");
        await load();
      } catch (e) {
        toast(e.message);
      }
      return;
    }
    const connection = new URLSearchParams(location.search).get("connection");
    if (connection) {
      toast(
        connection === "connected"
          ? "Email account connected. Your first inbox check has started."
          : "Email connection was not completed. Please try again.",
      );
      history.replaceState(null, "", "/");
      state.view = "connections";
    }
    render();
  } catch (e) {
    if (e.status !== 401) toast(e.message);
    state.me = null;
    $("app").hidden = true;
    $("auth").hidden = false;
  }
}
function renderUser() {
  $("workspace-select").innerHTML = state.me.workspaces
    .map(
      (w) =>
        `<option value="${esc(w.id)}" ${w.id === state.me.workspace ? "selected" : ""}>${esc(w.name)}</option>`,
    )
    .join("");
  $("preview-label").hidden = !state.me.is_sample;
  $("top-bar").hidden = !state.me.is_sample;
  $("role-label").textContent =
    state.me.role === "owner" ? "Workspace owner" : "Workspace member";
  $("user-avatar").textContent = state.me.user.email.slice(0, 1).toUpperCase();
  $("user-email").textContent = state.me.user.email;
}
async function refresh() {
  const request = ++state.refreshId;
  const params = new URLSearchParams({
    page: state.page,
    view: ["invoices", "review", "archive"].includes(state.view)
      ? state.view
      : "invoices",
    store: state.store,
    search: state.search,
    supplier: state.supplier,
    status: state.status,
    currency: state.currency,
    from: state.from,
    to: state.to,
    sort: state.sort,
  });
  const [records, connections] = await Promise.all([
    api("/api/invoices?" + params),
    api("/api/connections"),
  ]);
  if (request !== state.refreshId) return;
  state.records = records.invoices;
  state.total = records.total;
  state.counts = records.global_counts;
  state.connections = connections;
  state.suppliers = records.suppliers;
  state.currencies = records.currencies;
  state.currencyOptions = records.currency_options;
  $("review-badge").textContent = records.global_counts.review || "";
}
async function updateList() {
  try {
    await refresh();
    if (["invoices", "review", "archive"].includes(state.view)) {
      renderRows();
      updateTotals();
    }
  } catch (e) {
    toast(e.message);
  }
}
const viewCopy = {
  invoices: ["Invoices", "A clean record for every store."],
  review: [
    "Needs review",
    "Confirm uncertain fields and choose the right store.",
  ],
  archive: [
    "Document archive",
    "Find and download the original documents whenever you need them.",
  ],
  stores: [
    "Stores",
    "Route invoices using the store details already printed on them.",
  ],
  connections: [
    "Email connections",
    "Use the email addresses your suppliers already know.",
  ],
  team: [
    "Team access",
    "Give teammates access to this workspace and its stores.",
  ],
  privacy: [
    "Privacy & consent",
    "Understand what is stored and manage your permissions.",
  ],
};
function render() {
  if (!state.me) return;
  document
    .querySelectorAll("[data-view]")
    .forEach((b) =>
      b.classList.toggle("active", b.dataset.view === state.view),
    );
  const [title, description] = viewCopy[state.view];
  $("page-title").textContent = title;
  $("page-description").textContent = description;

  $("connect-email").textContent = state.connections?.connections.length
    ? "Email settings"
    : "Connect email";
  $("header-actions").hidden = !["invoices", "archive"].includes(state.view);
  if (["invoices", "review", "archive"].includes(state.view)) renderInvoices();
  else if (state.view === "stores") renderStores();
  else if (state.view === "connections") renderConnections();
  else if (state.view === "team") renderTeam();
  else renderPrivacy();
}
const localDay = (d) =>
  `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
function choosePeriod(period) {
  state.period = period;
  const now = new Date(),
    year = now.getFullYear(),
    month = now.getMonth();
  const bounds = {
    all: ["", ""],
    month: [new Date(year, month, 1), new Date(year, month + 1, 0)],
    last_month: [new Date(year, month - 1, 1), new Date(year, month, 0)],
    year: [new Date(year, 0, 1), new Date(year, 11, 31)],
    last_year: [new Date(year - 1, 0, 1), new Date(year - 1, 11, 31)],
  };
  if (bounds[period])
    [state.from, state.to] = bounds[period].map((d) => (d ? localDay(d) : ""));
}
function updateTotals() {
  if (!$("period-totals")) return;
  $("period-totals").classList.toggle(
    "closing-rule",
    state.currencies.length > 0,
  );
  $("period-totals").innerHTML = state.currencies.length
    ? state.currencies
        .map(
          (c) =>
            `<div class="currency-total"><strong>${esc(amount(c.total_cents, c.currency))}<small>${esc(c.currency)}</small></strong><span>${c.invoice_count} ${c.invoice_count === 1 ? "invoice" : "invoices"} · ${esc(amount(c.tax_cents, c.currency))} tax included</span></div>`,
        )
        .join("")
    : '<p class="secondary-text">No validated invoices in this selection.</p>';
}
function renderInvoices() {
  const c = state.counts;
  const total = Object.entries(c).reduce(
    (n, [status, count]) => n + (status === "bundle" ? 0 : count),
    0,
  );
  const posted = (c.ready || 0) + (c.approved || 0);
  $("page-content").innerHTML =
    `${state.view === "invoices" ? `<section class="stats" aria-label="Invoice overview"><div class="stat"><div class="stat-label">Invoices captured</div><div class="stat-value">${total}</div><div class="stat-note">Across ${state.me.stores.length} ${state.me.stores.length === 1 ? "store" : "stores"}</div></div><div class="stat accent"><div class="stat-label">Ready to export</div><div class="stat-value">${posted}</div><div class="stat-note">Validated invoice records</div></div><button class="stat review-stat ${c.review ? "" : "caught-up"}" id="review-shortcut"><div class="stat-label">Needs your review</div><div class="stat-value">${c.review || 0}</div><div class="stat-note">${c.review ? "Review invoices →" : "All caught up"}</div></button><div class="stat"><div class="stat-label">Processing</div><div class="stat-value">${c.processing || 0}</div><div class="stat-note">${c.processing ? "We’ll update these shortly" : "No invoices waiting"}</div></div></section>` : ""}
  ${state.view === "invoices" ? `<section class="period-summary" aria-label="Invoice totals"><div class="period-heading"><div><h3>Invoice totals</h3><p>Validated invoices · based on invoice date</p></div><label class="compact-label">Time period<select id="period-filter"><option value="all">All time</option><option value="month">This month</option><option value="last_month">Last month</option><option value="year">This year</option><option value="last_year">Last year</option><option value="custom">Custom dates</option></select></label></div><div id="period-totals" class="closing-rule"></div></section>` : ""}
  ${state.view !== "archive" ? `<section id="drop-zone" class="upload-panel"><div class="upload-icon">↑</div><div><h3>Add invoice PDFs</h3><p>Drop files here · up to 20 PDFs, 20 MB each</p></div><button id="upload-button" class="secondary" ${state.uploading ? "disabled" : ""}>Choose files</button></section>` : ""}
  <section class="table-card"><div class="table-toolbar"><h3>${state.view === "archive" ? "Document archive" : state.view === "review" ? "Waiting for review" : "All invoices"} <small id="record-count"></small></h3><div class="table-controls"><input id="search" type="search" aria-label="Search invoices" placeholder="Search supplier, number or PO" value="${esc(state.search)}"><select id="store-filter" aria-label="Filter by store"><option value="">All stores</option><option value="unassigned" ${state.store === "unassigned" ? "selected" : ""}>Unassigned</option>${state.me.stores.map((s) => `<option value="${esc(s.id)}" ${s.id === state.store ? "selected" : ""}>${esc(s.name)}</option>`).join("")}</select></div></div><div class="filter-bar"><label>Supplier<select id="supplier-filter"><option value="">All suppliers</option>${state.suppliers.map((name) => `<option value="${esc(name)}" ${name === state.supplier ? "selected" : ""}>${esc(name)}</option>`).join("")}</select></label><label>Status<select id="status-filter"><option value="">All statuses</option>${[
    ["posted", "Ready to export"],
    ["review", "Needs review"],
    ["processing", "Processing"],
    ["excluded", "Excluded"],
  ]
    .map(
      ([value, label]) =>
        `<option value="${value}" ${value === state.status ? "selected" : ""}>${label}</option>`,
    )
    .join(
      "",
    )}</select></label><label>Currency<select id="currency-filter"><option value="">All currencies</option>${state.currencyOptions.map((currency) => `<option ${currency === state.currency ? "selected" : ""}>${esc(currency)}</option>`).join("")}</select></label><label>Sort by<select id="sort-filter">${[
    ["received_newest", "Recently captured"],
    ["invoice_newest", "Invoice date: newest"],
    ["invoice_oldest", "Invoice date: oldest"],
    ["supplier_az", "Supplier: A–Z"],
    ["total_high", "Total: high to low (per currency)"],
    ["total_low", "Total: low to high (per currency)"],
  ]
    .map(
      ([value, label]) =>
        `<option value="${value}" ${value === state.sort ? "selected" : ""}>${label}</option>`,
    )
    .join(
      "",
    )}</select></label><label>From<input id="date-from" type="date" value="${esc(state.from)}"></label><label>To<input id="date-to" type="date" value="${esc(state.to)}"></label><button id="clear-filters" class="text-link">Reset filters</button></div><p id="filter-error" class="error filter-error" role="alert"></p><div id="invoice-table"></div><div class="table-footer"><span id="page-range"></span><div class="pagination"><button id="previous-page" class="icon-button" aria-label="Previous page">←</button><button id="next-page" class="icon-button" aria-label="Next page">→</button></div></div></section>`;
  renderRows();
  updateTotals();
  if ($("period-filter")) {
    $("period-filter").value = state.period;
    $("period-filter").onchange = async (e) => {
      choosePeriod(e.target.value);
      state.page = 1;
      $("date-from").value = state.from;
      $("date-to").value = state.to;
      $("filter-error").textContent = "";
      await updateList();
    };
  }
  for (const [id, key] of [
    ["supplier-filter", "supplier"],
    ["status-filter", "status"],
    ["currency-filter", "currency"],
    ["sort-filter", "sort"],
  ])
    $(id).onchange = async (e) => {
      state[key] = e.target.value;
      state.page = 1;
      await updateList();
    };
  for (const [id, key] of [
    ["date-from", "from"],
    ["date-to", "to"],
  ])
    $(id).onchange = async (e) => {
      state[key] = e.target.value;
      state.period = "custom";
      if ($("period-filter")) $("period-filter").value = "custom";
      state.page = 1;
      if (state.from && state.to && state.from > state.to) {
        $("filter-error").textContent =
          "The start date must be before the end date.";
        return;
      }
      $("filter-error").textContent = "";
      updateList();
    };
  $("clear-filters").onclick = async () => {
    state.supplier = "";
    state.status = "";
    state.currency = "";
    state.search = "";
    state.store = "";
    state.sort = "received_newest";
    state.page = 1;
    choosePeriod("all");
    try {
      await refresh();
      render();
    } catch (e) {
      toast(e.message);
    }
  };
  $("search").oninput = (e) => {
    state.search = e.target.value;
    state.page = 1;
    clearTimeout(state.searchTimer);
    state.refreshId++;
    state.searchTimer = setTimeout(updateList, 300);
  };
  $("store-filter").onchange = (e) => {
    state.store = e.target.value;
    state.page = 1;
    updateList();
  };
  $("previous-page").onclick = () => {
    state.page--;
    updateList();
  };
  $("next-page").onclick = () => {
    state.page++;
    updateList();
  };
  if ($("review-shortcut"))
    $("review-shortcut").onclick = () => navigate("review");
  if ($("upload-button"))
    $("upload-button").onclick = () => $("file-input").click();
  const zone = $("drop-zone");
  if (zone) {
    zone.ondragover = (e) => {
      e.preventDefault();
      zone.classList.add("drag-over");
    };
    zone.ondragleave = () => zone.classList.remove("drag-over");
    zone.ondrop = (e) => {
      e.preventDefault();
      zone.classList.remove("drag-over");
      uploadFiles(e.dataTransfer.files);
    };
  }
}
function renderRows() {
  const rows = state.records;
  $("record-count").textContent = state.total;
  $("page-range").textContent = state.total
    ? `${(state.page - 1) * 50 + 1}–${Math.min(state.page * 50, state.total)} of ${state.total}`
    : "0 invoices";
  $("previous-page").disabled = state.page <= 1;
  $("next-page").disabled = state.page * 50 >= state.total;
  if (!rows.length) {
    $("invoice-table").innerHTML =
      `<div class="empty"><h3>${state.search || state.store || state.supplier || state.status || state.currency || state.from || state.to ? "No matching invoices" : state.view === "review" ? "All caught up" : "Your invoices start here"}</h3><p>${state.search || state.store || state.supplier || state.status || state.currency || state.from || state.to ? "Try another filter or choose Reset filters." : state.view === "review" ? "There are no invoices waiting for review." : "Upload your first PDF or connect your existing email."}</p></div>`;
    return;
  }
  $("invoice-table").innerHTML =
    `<div class="table-wrap"><table><thead><tr><th>Supplier</th><th>Store</th><th>Invoice date</th><th>Total</th><th>Status</th><th>Source</th><th><span class='sr-only'>Open</span></th></tr></thead><tbody>${rows.map((r) => `<tr data-record="${esc(r.id)}" tabindex="0" aria-label="Open invoice ${esc(r.invoice.invoice_number || r.file_name)}"><td><span class="supplier-avatar">${esc((r.invoice.supplier || "PDF").slice(0, 2).toUpperCase())}</span><div class="supplier">${esc(r.invoice.supplier || r.file_name)}<small>${esc(r.invoice.invoice_number || "Awaiting extraction")}</small></div></td><td>${esc(storeName(r.store_id))}</td><td>${esc(date(r.invoice.invoice_date))}</td><td class="money">${r.invoice.currency ? esc(amount(r.invoice.total_cents, r.invoice.currency)) + (state.currencyOptions.length > 1 ? ` <small>${esc(r.invoice.currency)}</small>` : "") : r.invoice.total_cents == null ? "—" : esc(money(r.invoice.total_cents)) + " <small>currency?</small>"}</td><td>${badge(r)}</td><td>${esc(r.source)}</td><td class="row-arrow">→</td></tr>`).join("")}</tbody></table></div>`;
  document.querySelectorAll("[data-record]").forEach((row) => {
    row.onclick = () => openRecord(row.dataset.record);
    row.onkeydown = (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        openRecord(row.dataset.record);
      }
    };
  });
}
async function uploadFiles(fileList) {
  if (state.uploading) {
    toast("Your upload is still in progress.");
    return;
  }
  const files = [...fileList];
  if (!files.length) return;
  if (
    files.length > 20 ||
    files.some(
      (f) =>
        f.size > 20 * 1024 * 1024 || !f.name.toLowerCase().endsWith(".pdf"),
    )
  ) {
    toast("Choose up to 20 PDFs, 20 MB each.");
    return;
  }
  state.uploading = true;
  let added = 0,
    duplicates = 0;
  const failed = [];
  if ($("upload-button")) $("upload-button").disabled = true;
  for (let index = 0; index < files.length; index++) {
    toast(`Uploading ${index + 1} of ${files.length}…`);
    const form = new FormData();
    form.append("files", files[index]);
    try {
      const result = await api("/api/upload", { method: "POST", body: form });
      for (const file of result.files) {
        if (file.duplicate) duplicates++;
        else added++;
      }
    } catch (e) {
      failed.push(files[index].name);
    }
  }
  state.uploading = false;
  $("file-input").value = "";
  await updateList();
  render();
  toast(
    `${added} added${duplicates ? ` · ${duplicates} already captured` : ""}${failed.length ? ` · ${failed.length} failed. Try these again: ${failed.join(", ")}` : ""}`,
  );
}
async function openRecord(id) {
  if (!closeDrawer()) return;
  const request = ++state.drawerRequest;
  let r;
  try {
    r = (await api("/api/invoices/" + encodeURIComponent(id))).invoice;
  } catch (e) {
    toast(e.message);
    return;
  }
  if (request !== state.drawerRequest || !state.me) return;
  state.drawerDirty = false;
  state.returnFocus = document.activeElement;
  state.selected = id;
  const i = r.invoice;
  $("drawer").innerHTML =
    `<div class="drawer-heading"><div><h2>${esc(i.supplier || "Review invoice")}</h2><small>${esc(i.invoice_number || r.file_name)}</small></div><button id="close-drawer" class="icon-button" aria-label="Close invoice">×</button></div>${badge(r)}${["ready", "approved"].includes(r.status) && i.total_cents != null ? `<div class="checked-total"><small>Invoice total</small><strong class="closing-rule">${esc(amount(i.total_cents, i.currency))}<small>${esc(i.currency)}</small></strong></div>` : ""}<details class="source-disclosure"><summary>Document details</summary><div class="source-details"><div><small>Source</small><strong>${esc(r.source)}${r.email_sender ? " · " + esc(r.email_sender) : ""}</strong></div><div><small>Imported</small><strong>${esc(date(r.imported_at))}</strong></div><div><small>Pages</small><strong>${i.pages || "—"}</strong></div></div></details>${r.email_subject ? `<p class="drawer-note">Email: ${esc(r.email_subject)}</p>` : ""}<a class="secondary" href="/api/invoices/${encodeURIComponent(id)}/document"><svg class="status-icon" viewBox="0 0 20 20" aria-hidden="true"><use href="#t-source"></use></svg> Original PDF</a>${r.original_document_id ? ` <a class="text-link" href="/api/invoices/${encodeURIComponent(r.original_document_id)}/document">Full source attachment</a>` : ""}${i.warnings.length ? `<div class="info-box warn"><ul class="warning-list">${i.warnings.map((w) => `<li>${esc(w)}</li>`).join("")}</ul></div>` : ""}${["processing", "bundle"].includes(r.status) ? `<p class="drawer-note">${r.status === "processing" ? "This document is being processed. Close this panel and check again shortly." : "Each invoice in this attachment has its own record. Review those records independently."}</p>` : `<form id="review-form"><div class="field-grid"><label class="full">Store<select name="store_id" required><option value="">Choose a store</option>${state.me.stores.map((s) => `<option value="${esc(s.id)}" ${s.id === r.store_id ? "selected" : ""}>${esc(s.name)}</option>`).join("")}</select></label><label class="full">Supplier<input name="supplier" value="${esc(i.supplier)}" maxlength="200" required></label><label>Invoice number<input name="invoice_number" value="${esc(i.invoice_number)}" maxlength="100" required></label><label>PO number<input name="po_number" value="${esc(i.po_number)}"></label><label>Invoice date<input name="invoice_date" type="date" value="${esc(i.invoice_date)}" required></label><label>Due date<input name="due_date" type="date" value="${esc(i.due_date)}"></label><label>Currency<input name="currency" value="${esc(i.currency)}" placeholder="CAD" maxlength="3" required></label><label>Subtotal<input name="subtotal" inputmode="decimal" value="${money(i.subtotal_cents)}" required></label><label>Tax<input name="tax" inputmode="decimal" value="${money(i.tax_cents)}" required></label><label>Total<input name="total" inputmode="decimal" value="${money(i.total_cents)}" required></label><label>Balance shown on invoice<input name="balance" inputmode="decimal" value="${money(i.balance_cents)}"></label></div><p id="review-error" class="error" role="alert"></p><div class="drawer-buttons"><button class="primary" type="submit">Save invoice</button><button class="secondary" type="submit" value="next">Save & next</button><button id="exclude" class="secondary" type="button">Exclude</button></div><p class="drawer-note">The original document is preserved. An invoice's stated balance does not confirm whether it has since been paid.</p></form>`}${i.items.length ? `<details class="items-list"><summary>Line items · ${i.items.length}</summary>${i.items.map((item) => `<div class="item-row"><span>${esc(item.description)}<small>${item.sku ? esc(item.sku) + " · " : ""}Qty ${esc(item.quantity || "—")}</small></span><span>${esc(money(item.amount_cents))}</span></div>`).join("")}</details>` : ""}${r.status !== "processing" ? `<button id="delete-invoice" class="text-link delete-link">Delete from archive</button>` : ""}`;
  $("drawer").hidden = false;
  $("drawer").showModal();
  $("close-drawer").onclick = closeDrawer;
  $("close-drawer").focus();
  if ($("review-form"))
    $("review-form").oninput = () => {
      state.drawerDirty = true;
    };
  if ($("review-form"))
    $("review-form").onsubmit = async (e) => {
      e.preventDefault();
      const button = e.submitter;
      button.disabled = true;
      try {
        const values = Object.fromEntries(new FormData(e.target));
        values.revision = r.revision;
        const key =
          state.saveIntent && state.saveIntent.body === JSON.stringify(values)
            ? state.saveIntent.key
            : crypto.randomUUID();
        state.saveIntent = { body: JSON.stringify(values), key };
        await api(`/api/invoices/${id}/approve`, {
          method: "POST",
          headers: { "Idempotency-Key": key },
          body: values,
        });
        state.saveIntent = null;
        state.drawerDirty = false;
        toast("Invoice saved.");
        closeDrawer();
        await refresh();
        render();
        if (button.value === "next") {
          const next = state.records.find(
            (item) => item.status === "review" && item.id !== id,
          );
          if (next) openRecord(next.id);
        }
      } catch (e) {
        $("review-error").textContent = e.message;
      } finally {
        button.disabled = false;
      }
    };
  if ($("exclude"))
    $("exclude").onclick = async () => {
      try {
        await api(`/api/invoices/${id}/exclude`, { method: "POST" });
        state.drawerDirty = false;
        closeDrawer();
        await refresh();
        render();
        toast("Excluded from the spreadsheet. Original PDF retained.");
      } catch (e) {
        toast(e.message);
      }
    };
  if ($("delete-invoice"))
    $("delete-invoice").onclick = async () => {
      if (
        !confirm(
          r.original_document_id
            ? "Delete this invoice record and its extracted PDF? The full source attachment still contains it; delete that attachment separately to remove it from the active archive. Downloaded copies remain."
            : "Delete this record and its PDF from the active archive? Separately extracted child invoices and previously downloaded copies will remain.",
        )
      )
        return;
      try {
        await api(`/api/invoices/${id}/delete`, { method: "POST" });
        state.drawerDirty = false;
        closeDrawer();
        await refresh();
        render();
        toast("Invoice deleted.");
      } catch (e) {
        toast(e.message);
      }
    };
}
function closeDrawer() {
  if (state.drawerDirty && !confirm("Discard your unsaved changes?"))
    return false;
  state.drawerRequest++;
  state.drawerDirty = false;
  state.saveIntent = null;
  if ($("drawer").open) $("drawer").close();
  $("drawer").hidden = true;
  $("drawer-backdrop").hidden = true;
  state.selected = null;
  if (state.returnFocus?.isConnected) state.returnFocus.focus();
  return true;
}
async function navigate(view) {
  closeNavigation();
  closeAccountMenu();
  if (!closeDrawer()) return;
  state.view = view;
  state.page = 1;
  state.search = "";
  try {
    await refresh();
    render();
  } catch (e) {
    toast(e.message);
  }
}
function renderStores() {
  const canEdit = state.me.role === "owner";
  $("page-content").innerHTML =
    `<div class="info-box">A single mailbox can serve all your stores. Use specific identifiers from invoices, such as a street address or store number. Conflicting or missing matches stay unassigned for review.</div><div class="card-grid"><div><h3>Your stores</h3>${state.me.stores.map((s) => `<section class="card"><form data-store-form="${esc(s.id)}"><label>Store name<input name="name" value="${esc(s.name)}" required ${canEdit ? "" : "disabled"}></label><label>Invoice identifiers<textarea name="identifiers" placeholder="2500 Douglas\nStore #25" ${canEdit ? "" : "disabled"}>${esc(s.identifiers.join("\n"))}</textarea></label><p class="secondary-text">One specific phrase per line, at least 8 characters. Avoid city names or supplier names.</p>${canEdit ? '<button class="secondary">Save store</button>' : ""}</form></section>`).join("")}</div>${canEdit ? `<section class="card"><h3>Add a store</h3><p>Keep each location's invoices and spreadsheet exports separate.</p><form id="add-store-form"><label>Store name<input name="name" required maxlength="100" placeholder="Downtown store"></label><label>Invoice identifiers<textarea name="identifiers" placeholder="123 Main Street"></textarea></label><button class="primary">Add store</button></form></section>` : ""}</div>`;
  document.querySelectorAll("[data-store-form]").forEach(
    (form) =>
      (form.onsubmit = async (e) => {
        e.preventDefault();
        const values = Object.fromEntries(new FormData(form));
        values.identifiers = values.identifiers
          .split("\n")
          .map((v) => v.trim())
          .filter(Boolean);
        try {
          const result = await api("/api/stores/" + form.dataset.storeForm, {
            method: "POST",
            body: values,
          });
          state.me.stores = result.stores;
          toast("Store identifiers saved.");
          render();
        } catch (e) {
          toast(e.message);
        }
      }),
  );
  if ($("add-store-form"))
    $("add-store-form").onsubmit = async (e) => {
      e.preventDefault();
      const values = Object.fromEntries(new FormData(e.target));
      values.identifiers = values.identifiers
        .split("\n")
        .map((v) => v.trim())
        .filter(Boolean);
      try {
        const result = await api("/api/stores", {
          method: "POST",
          body: values,
        });
        state.me.stores = result.stores;
        toast("Store added.");
        render();
      } catch (e) {
        toast(e.message);
      }
    };
}
function renderConnections() {
  $("page-content").innerHTML =
    `<div class="info-box">Suppliers keep sending to your existing address. Each PDF attachment is processed separately, and invoices can be routed to different stores from the same email.</div><div class="card-grid">${[
      ["google", "Gmail / Google Workspace", "G"],
      ["microsoft", "Outlook / Microsoft 365", "M"],
    ]
      .map(([provider, title, letter]) => {
        const connection = state.connections.connections.find(
          (c) => c.provider === provider,
        );
        const available =
          state.connections.available[provider] && !state.me.is_sample;
        return `<section class="card"><div class="email-icon">${letter}</div><h3>${title}</h3>${connection ? `<p>${esc(connection.email)}</p><div class="connection-meta">Last checked: ${esc(connection.last_sync ? new Date(connection.last_sync).toLocaleString() : "First check pending")}<br>${connection.last_error ? esc(connection.last_error) : "Automatic checks every 5 minutes."}</div><div class="row"><button class="primary sync-button">Check inbox now</button>${state.me.role === "owner" ? `<button class="secondary disconnect-button" data-provider="${provider}">Disconnect</button>` : ""}</div>` : `<p>Connect an account already receiving your supplier invoices.</p><div class="connection-meta">Mail access is read-only and can cover the selected mailbox. Only candidate PDFs and source metadata are retained; email bodies are not stored.</div>${!available ? `<div class="info-box warn">${state.me.is_sample ? "Create your own workspace to connect your mailbox." : "Email connection is currently unavailable. You can still upload PDFs."}</div>` : ""}<label class="check-label"><input type="checkbox" id="consent-${provider}" ${available && state.me.role === "owner" ? "" : "disabled"}><span>I authorize reading this mailbox for invoice capture and sharing its imported documents with this workspace. I have reviewed the <button class="text-link policy-link" type="button">privacy notice</button>.</span></label><button class="primary connect-provider" data-provider="${provider}" ${available && state.me.role === "owner" ? "" : "disabled"}>Connect ${provider === "google" ? "Google" : "Microsoft"} account</button>`}</section>`;
      })
      .join(
        "",
      )}</div><div class="card"><h3>What happens after connecting?</h3><p>The first check looks back ${state.me.initial_lookback_days} ${state.me.initial_lookback_days === 1 ? "day" : "days"}. Later checks overlap safely and skip documents already received. Each invoice is stored with its source PDF and email reference. Unclear store assignments or extracted fields are held for review.</p><p>Disconnecting stops future checks and removes locally stored credentials. Imported invoices remain in your archive. You can also remove the app in your Google or Microsoft account permissions.</p></div>`;
  bindPolicies();
  document.querySelectorAll(".connect-provider").forEach(
    (b) =>
      (b.onclick = async () => {
        const provider = b.dataset.provider;
        if (!$("consent-" + provider).checked) {
          toast("Review and consent to email processing first.");
          return;
        }
        b.disabled = true;
        try {
          const result = await api(`/api/connections/${provider}/start`, {
            method: "POST",
            body: { consent: true, policy_version: state.me.policy_version },
          });
          location.assign(result.url);
        } catch (e) {
          toast(e.message);
          b.disabled = false;
        }
      }),
  );
  document.querySelectorAll(".sync-button").forEach(
    (b) =>
      (b.onclick = async () => {
        try {
          await api("/api/sync", { method: "POST" });
          toast("Inbox check started.");
        } catch (e) {
          toast(e.message);
        }
      }),
  );
  document.querySelectorAll(".disconnect-button").forEach(
    (b) =>
      (b.onclick = async () => {
        try {
          await api(`/api/connections/${b.dataset.provider}/disconnect`, {
            method: "POST",
          });
          await refresh();
          render();
          toast("Email disconnected. Existing invoices are retained.");
        } catch (e) {
          toast(e.message);
        }
      }),
  );
}
async function renderTeam() {
  $("page-content").innerHTML =
    `<div class="info-box">Workspace members can access all stores, original invoices, and spreadsheets in this workspace. Owners manage stores, email connections, and invitations. Invite only people authorized to see these documents.</div><section class="card"><h3>Invite a teammate</h3>${state.me.role === "owner" ? `<form id="invite-form"><label>Teammate's email<input name="email" type="email" required></label><button class="primary">Create invitation link</button></form><p class="secondary-text">The link works for this email only and expires after 7 days. Share it directly with your teammate.</p><div id="invite-result" hidden></div>` : "<p>Ask a workspace owner to invite a teammate.</p>"}</section><section class="card"><h3>Workspace members</h3><div id="members-list">Loading…</div></section>`;
  try {
    const data = await api("/api/members");
    if (state.view !== "team") return;
    $("members-list").innerHTML = data.members
      .map(
        (m) =>
          `<div class="audit-row">${esc(m.email)}<small>${esc(m.role)}</small>${state.me.role === "owner" && m.role === "member" ? `<button class="text-link remove-member" data-member="${esc(m.id)}">Remove access</button>` : ""}</div>`,
      )
      .join("");
    document.querySelectorAll(".remove-member").forEach(
      (b) =>
        (b.onclick = async () => {
          if (
            !confirm(
              "Remove this teammate" +
                String.fromCharCode(39) +
                "s access to this workspace and its invoices?",
            )
          )
            return;
          try {
            await api(`/api/members/${b.dataset.member}/remove`, {
              method: "POST",
            });
            toast("Workspace access removed.");
            renderTeam();
          } catch (e) {
            toast(e.message);
          }
        }),
    );
  } catch (e) {
    toast(e.message);
  }
  if ($("invite-form"))
    $("invite-form").onsubmit = async (e) => {
      e.preventDefault();
      try {
        const result = await api("/api/invitations", {
          method: "POST",
          body: Object.fromEntries(new FormData(e.target)),
        });
        $("invite-result").hidden = false;
        $("invite-result").className = "invite-result";
        $("invite-result").textContent = result.url;
      } catch (e) {
        toast(e.message);
      }
    };
}
async function renderPrivacy() {
  $("page-content").innerHTML =
    `<section class="card"><h3>Your documents and permissions</h3><ul class="privacy-summary"><li>Email connection is optional. PDF upload works without mailbox access.</li><li>Original invoices and extracted records are retained so authorized workspace members can access them.</li><li>Documents are processed without external AI services or advertising analytics.</li><li>You can correct fields, download PDFs and spreadsheets, delete invoices, and disconnect email.</li></ul><button class="secondary policy-link">Read the full privacy notice</button></section><section class="card"><h3>Your consent history</h3><div id="consent-history">Loading…</div></section>`;
  bindPolicies();
  try {
    const data = await api("/api/consents");
    if (state.view !== "privacy") return;
    $("consent-history").innerHTML = data.consents.length
      ? data.consents
          .map(
            (c) =>
              `<div class="audit-row">${esc(c.kind.replaceAll("_", " "))}<small>Version ${esc(c.version)} · Granted ${esc(new Date(c.granted_at).toLocaleString())}${c.withdrawn_at ? " · Withdrawn " + esc(new Date(c.withdrawn_at).toLocaleString()) : ""}</small></div>`,
          )
          .join("")
      : '<p class="secondary-text">No consent records in this workspace yet.</p>';
  } catch (e) {
    toast(e.message);
  }
}
async function openPolicy() {
  try {
    state.policy = state.policy || (await api("/api/privacy"));
    const content = $("policy-content");
    content.replaceChildren();
    for (const paragraph of state.policy.notice.split("\n\n")) {
      const heading = paragraph.match(/^(#{1,2})\s+(.*)$/);
      const element = document.createElement(
        heading ? (heading[1].length === 1 ? "h1" : "h2") : "p",
      );
      element.textContent = heading ? heading[2] : paragraph;
      content.appendChild(element);
    }
    $("policy-dialog").showModal();
  } catch (e) {
    toast(e.message);
  }
}
function bindPolicies() {
  document
    .querySelectorAll(".policy-link")
    .forEach((b) => (b.onclick = openPolicy));
}
function authMode(mode) {
  state.mode = mode;
  const register = mode === "register";
  $("store-name-label").hidden = !register;
  $("account-consent-label").hidden = !register;
  $("auth-title").textContent = register
    ? "Create your workspace"
    : "Welcome back";
  $("auth-subtitle").textContent = register
    ? "Start with one store. Add more whenever you need."
    : "Sign in to your invoice records.";
  $("auth-submit").textContent = register ? "Create account" : "Sign in";
  $("password").autocomplete = register ? "new-password" : "current-password";
  $("password").minLength = register ? 12 : 1;
  $("login-tab").classList.toggle("active", !register);
  $("register-tab").classList.toggle("active", register);
  $("auth-error").textContent = "";
}
$("login-tab").onclick = () => authMode("login");
$("register-tab").onclick = () => authMode("register");
$("auth-form").onsubmit = async (e) => {
  e.preventDefault();
  $("auth-submit").disabled = true;
  $("auth-error").textContent = "";
  try {
    state.policy = state.policy || (await api("/api/privacy"));
    await api(
      "/api/auth/" + (state.mode === "register" ? "register" : "login"),
      {
        method: "POST",
        body: {
          email: $("email").value,
          password: $("password").value,
          store_name: $("store-name").value,
          consent: $("account-consent").checked,
          policy_version: state.policy.version,
        },
      },
    );
    $("password").value = "";
    await load();
  } catch (e) {
    $("auth-error").textContent = e.message;
  } finally {
    $("auth-submit").disabled = false;
  }
};
$("demo-button").onclick = async () => {
  try {
    $("demo-button").disabled = true;
    await api("/api/auth/demo", { method: "POST" });
    await load();
  } catch (e) {
    toast(e.message);
  } finally {
    $("demo-button").disabled = false;
  }
};
document
  .querySelectorAll("[data-view]")
  .forEach((b) => (b.onclick = () => navigate(b.dataset.view)));
$("connect-email").onclick = () => navigate("connections");
$("export").onclick = async () => {
  const params = new URLSearchParams({
    store: state.store,
    supplier: state.supplier,
    from: state.from,
    to: state.to,
    currency: state.currency,
    status: state.status,
    search: state.search,
    sort: state.sort,
  });
  $("export").disabled = true;
  try {
    const response = await fetch("/api/export?" + params);
    if (!response.ok) {
      const error = await response.json().catch(() => ({}));
      throw new Error(
        error.error ||
          "The spreadsheet could not be downloaded. Please try again.",
      );
    }
    const url = URL.createObjectURL(await response.blob());
    const link = document.createElement("a");
    link.href = url;
    link.download = `${PRODUCT_NAME}-invoices.xlsx`;
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 30000);
    toast("Spreadsheet download started.");
  } catch (e) {
    toast(
      e.message === "Failed to fetch"
        ? "Connection lost. Please retry the download."
        : e.message,
    );
  } finally {
    $("export").disabled = false;
  }
};
$("logout").onclick = () => {
  closeAccountMenu();
  $("signout-error").textContent = "";
  $("signout-message").textContent = state.drawerDirty
    ? "Your unsaved invoice changes will be discarded. You’ll need to sign in again to access your workspace."
    : "You’ll need to sign in again to access your workspace.";
  $("signout-dialog").showModal();
};
$("cancel-signout").onclick = () => $("signout-dialog").close();
$("signout-dialog").addEventListener("cancel", (e) => {
  if ($("confirm-signout").disabled) e.preventDefault();
});
$("confirm-signout").onclick = async () => {
  $("confirm-signout").disabled = true;
  $("cancel-signout").disabled = true;
  try {
    await api("/api/auth/logout", { method: "POST" });
    $("signout-dialog").close();
    state.drawerDirty = false;
    closeDrawer();
    state.records = [];
    state.selected = null;
    state.store = "";
    state.supplier = "";
    state.currency = "";
    state.status = "";
    state.search = "";
    state.page = 1;
    state.period = "all";
    state.from = "";
    state.to = "";
    state.view = "invoices";
    await load();
  } catch (e) {
    $("signout-error").textContent = e.message;
  } finally {
    $("confirm-signout").disabled = false;
    $("cancel-signout").disabled = false;
  }
};
$("workspace-select").onchange = async (e) => {
  closeNavigation();
  const workspace = e.target.value;
  if (!closeDrawer()) {
    e.target.value = state.me.workspace;
    return;
  }
  try {
    await api(`/api/workspaces/${workspace}/select`, { method: "POST" });
    state.store = "";
    state.supplier = "";
    state.currency = "";
    state.status = "";
    state.search = "";
    state.page = 1;
    closeDrawer();
    await load();
  } catch (e) {
    e.target.value = state.me.workspace;
    toast(e.message);
  }
};
$("file-input").onchange = (e) => uploadFiles(e.target.files);
$("drawer-backdrop").onclick = closeDrawer;
$("drawer").addEventListener("cancel", (e) => {
  e.preventDefault();
  closeDrawer();
});
$("close-policy").onclick = () => $("policy-dialog").close();
bindPolicies();
api("/api/privacy")
  .then((p) => {
    state.policy = p;
    $("demo-area").hidden = !p.demo;
  })
  .catch(() => {});
load();
setInterval(async () => {
  if (
    document.hidden ||
    !state.me ||
    !$("drawer").hidden ||
    state.uploading ||
    !["invoices", "review", "archive"].includes(state.view) ||
    document.activeElement?.id === "search"
  )
    return;
  try {
    const before = JSON.stringify([state.records, state.counts]);
    await refresh();
    if (before !== JSON.stringify([state.records, state.counts])) render();
  } catch (e) {
    if (e.status === 401) load();
  }
}, 15000);
document.addEventListener("visibilitychange", () => {
  if (!document.hidden && state.me && $("drawer").hidden) updateList();
});
