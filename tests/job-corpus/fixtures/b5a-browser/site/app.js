/* Parts Depot front end.
 *
 * The order token is derived from where the handle actually is on screen, so a
 * quantity that was never dragged into place cannot produce a valid order.
 */
(function () {
  "use strict";

  var state = { value: 0, nonce: null };
  var $ = function (id) { return document.getElementById(id); };

  function post(path, body) {
    return fetch(path, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: JSON.stringify(body || {})
    }).then(function (r) { return r.json(); });
  }

  function note(type, extra) {
    var payload = { type: type };
    if (extra) { Object.keys(extra).forEach(function (k) { payload[k] = extra[k]; }); }
    return post("/event", payload);
  }

  async function sha256Hex(text) {
    var buf = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
    return Array.prototype.map
      .call(new Uint8Array(buf), function (b) { return b.toString(16).padStart(2, "0"); })
      .join("");
  }

  function place(value) {
    state.value = Math.max(0, Math.min(100, Math.round(value)));
    $("handle").style.left = (state.value / 100 * 380) + "px";
    $("handle").setAttribute("aria-valuenow", String(state.value));
    $("qty").textContent = String(state.value);
  }

  $("login").addEventListener("click", function () {
    post("/login", { user: $("user").value, password: $("pass").value }).then(function (r) {
      if (!r.ok) { $("status").textContent = "Sign-in failed."; return; }
      state.nonce = r.nonce;
      $("login-box").classList.add("hidden");
      $("order-box").classList.remove("hidden");
      $("status").textContent = "Signed in. Load the catalogue.";
    });
  });

  $("load").addEventListener("click", function () {
    fetch("/catalogue", { credentials: "same-origin" })
      .then(function (r) { return r.json(); })
      .then(function (r) {
        var sel = $("part");
        sel.innerHTML = "";
        r.parts.forEach(function (p) {
          var o = document.createElement("option");
          o.value = p.id;
          o.textContent = p.id + " — " + p.name;
          sel.appendChild(o);
        });
        $("status").textContent = "Catalogue loaded.";
      });
  });

  var dragging = false;
  var track = $("track");

  function fromPointer(ev) {
    var r = track.getBoundingClientRect();
    place((ev.clientX - r.left) / r.width * 100);
  }

  $("handle").addEventListener("pointerdown", function (ev) {
    dragging = true;
    $("handle").setPointerCapture(ev.pointerId);
    note("pointerdown", { trusted: ev.isTrusted });
  });
  window.addEventListener("pointermove", function (ev) {
    if (!dragging) { return; }
    fromPointer(ev);
    note("pointermove", { trusted: ev.isTrusted, value: state.value });
  });
  window.addEventListener("pointerup", function (ev) {
    if (!dragging) { return; }
    dragging = false;
    note("pointerup", { trusted: ev.isTrusted, value: state.value });
  });
  $("handle").addEventListener("keydown", function (ev) {
    var delta = ev.key === "ArrowRight" ? 1 : ev.key === "ArrowLeft" ? -1 : 0;
    if (!delta) { return; }
    ev.preventDefault();
    place(state.value + delta);
    note("keydown", { trusted: ev.isTrusted, value: state.value });
  });

  $("expedite").addEventListener("change", function (ev) {
    note("change", { trusted: ev.isTrusted, expedite: $("expedite").checked });
  });

  $("submit").addEventListener("click", async function () {
    var hr = $("handle").getBoundingClientRect();
    var tr = track.getBoundingClientRect();
    var offset = Math.round(hr.left - tr.left);
    var part = $("part").value;
    var expedite = $("expedite").checked;
    var token = await sha256Hex([state.nonce, offset, part, expedite ? "1" : "0"].join(":"));
    var r = await post("/submit", {
      part: part, expedite: expedite, offset_px: offset,
      displayed_quantity: state.value, token: token
    });
    $("status").textContent = r.ok
      ? "Order " + r.order_id + " placed."
      : "Rejected: " + r.error;
  });
})();
