// Mints a YouTube proof-of-origin token, inside a real browser engine, and leaves it in a cookie.
//
// This is the flow the YouTube web player runs itself. The page publishes a BotGuard challenge and
// its own configuration; the virtual machine behind that challenge attests the browser it is
// running in, Google trades the attestation for an integrity token, and the token yields a minter.
// Only a real engine passes the attestation, which is the whole reason this runs here rather than
// in the app's own javascript interpreter.
//
// The binding to mint for arrives in the query string and the answer goes back in a cookie,
// because reading cookies is all the host can do with the page.
(function () {
  "use strict";

  var COOKIE = "SONORA_POT";
  // What the host reads as a failure rather than a token. A token is base64url and never starts
  // with one of these.
  var FAILED = "!";
  // How long the challenge, and then the virtual machine, are waited for.
  var PATIENCE = 20000;
  var STEP = 100;

  function answer(value) {
    document.cookie = COOKIE + "=" + value + "; path=/; max-age=120";
  }

  function fail(why) {
    answer(
      FAILED +
        String(why)
          .slice(0, 180)
          .replace(/[;,\s]+/g, " "),
    );
  }

  // The binding, read the moment this script runs. The page rewrites its own url before it
  // finishes loading and navigates away from it afterwards, so it is read at document start and
  // kept in the tab's own storage for whatever page comes next.
  var wanted = (function () {
    var found = /[?&]binding=([^&]+)/.exec(location.search);
    var value = found ? decodeURIComponent(found[1]) : null;
    try {
      if (value) sessionStorage.setItem(COOKIE, value);
      else value = sessionStorage.getItem(COOKIE);
    } catch (error) {
      // A page that refuses storage still works on its first load.
    }
    return value;
  })();

  // Whether a page before this one already left a token. Only a failure is worth replacing.
  function minted() {
    var found = new RegExp("(?:^|; )" + COOKIE + "=([^;]*)").exec(
      document.cookie,
    );
    return found && found[1] && found[1].charAt(0) !== FAILED;
  }

  // Waits for `condition` to return something, checking every `STEP` until `PATIENCE` is up.
  function until(condition, why) {
    return new Promise(function (resolve, reject) {
      var waited = 0;
      (function look() {
        var found = condition();
        if (found) return resolve(found);
        if ((waited += STEP) > PATIENCE) return reject(new Error(why));
        setTimeout(look, STEP);
      })();
    });
  }

  // The challenge, taken as the page hands it over.
  //
  // YouTube publishes it by calling `window.ytAtN`, a resolver it creates for its own promise and
  // deletes on `DOMContentLoaded`, passing nothing when the page carries no challenge. Standing
  // between the two is the only way to see the payload whichever order those happen in: the page's
  // resolver goes in through the setter, and everyone who reads `window.ytAtN` gets a wrapper that
  // records the payload and passes it straight on.
  var captured = null;
  var resolver = null;

  function relay(payload) {
    if (payload) captured = payload;
    if (resolver) resolver(payload);
  }

  try {
    Object.defineProperty(window, "ytAtN", {
      configurable: true,
      get: function () {
        return resolver ? relay : undefined;
      },
      set: function (value) {
        resolver = value;
      },
    });
  } catch (error) {
    // A page that will not take the property still loads; it just never mints.
  }

  function challenge() {
    return until(function () {
      return captured;
    }, "the page published no botguard challenge").then(function (payload) {
      var response = payload.R;
      if (typeof response === "string") response = JSON.parse(response);
      var found = response && response.bgChallenge;
      if (!found || !found.program || !found.interpreterUrl) {
        throw new Error("the challenge carries no program");
      }
      return found;
    });
  }

  function base64ToBytes(value) {
    var binary = atob(value.replace(/-/g, "+").replace(/_/g, "/"));
    var bytes = new Uint8Array(binary.length);
    for (var at = 0; at < binary.length; at++)
      bytes[at] = binary.charCodeAt(at);
    return bytes;
  }

  function bytesToBase64(bytes) {
    var binary = "";
    for (var at = 0; at < bytes.length; at++)
      binary += String.fromCharCode(bytes[at]);
    return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_");
  }

  // The page's trusted types policy, which its own scripts go through. Youtube refuses a plain
  // string for a script url, so one has to be minted here too; a page that will not hand out a
  // policy leaves `source` returning the string and the assignment below fails loudly.
  var policy = (function () {
    if (!window.trustedTypes || !window.trustedTypes.createPolicy) return null;
    try {
      return window.trustedTypes.createPolicy("sonora-potoken", {
        createScriptURL: function (url) {
          return url;
        },
      });
    } catch (error) {
      return null;
    }
  })();

  function source(url) {
    return policy ? policy.createScriptURL(url) : url;
  }

  // Brings the interpreter in the way the page's own player does, as a script element. Fetching it
  // instead would need cross-origin permission google.com does not give, and running the source
  // through `new Function` would need the eval the page's content policy forbids.
  function load(url) {
    return new Promise(function (resolve, reject) {
      var element = document.createElement("script");
      element.onload = function () {
        resolve();
      };
      element.onerror = function () {
        reject(new Error("cannot load the botguard interpreter"));
      };
      try {
        element.src = source(url);
      } catch (error) {
        return reject(
          new Error("the page refused the interpreter url: " + error.message),
        );
      }
      document.head.appendChild(element);
    });
  }

  // Loads the virtual machine and takes one snapshot of it. The third element of the array handed
  // to the snapshot is where BotGuard leaves the factory that makes a minter, which is the part we
  // are here for; an engine it does not trust gets a snapshot and an empty array.
  function attest(bg) {
    var vm = window[bg.globalName];
    if (!vm || !vm.a)
      return Promise.reject(new Error("the interpreter defined no vm"));
    var functions = null;
    try {
      vm.a(
        bg.program,
        function (snapshot, shutdown, passEvent, checkCamera) {
          functions = {
            snapshot: snapshot,
            shutdown: shutdown,
            passEvent: passEvent,
            checkCamera: checkCamera,
          };
        },
        true,
        undefined,
        function () {},
        [[], []],
        undefined,
        false,
        undefined,
      );
    } catch (error) {
      return Promise.reject(new Error("the vm refused its program"));
    }
    return until(function () {
      return functions;
    }, "the vm never handed back its functions").then(function (ready) {
      return new Promise(function (resolve, reject) {
        var signals = [];
        var settled = false;
        setTimeout(function () {
          if (!settled) reject(new Error("the snapshot timed out"));
        }, PATIENCE);
        ready.snapshot(
          function (token) {
            settled = true;
            resolve({ token: token, signals: signals });
          },
          [undefined, undefined, signals, undefined],
        );
      });
    });
  }

  // The player script the page is running, which is where the attestation constants live. Its url
  // is in the page's own configuration, and it is same-origin, so the browser usually answers this
  // out of the cache it filled when the page loaded.
  function player() {
    var url =
      (window.yt && window.yt.config_ && window.yt.config_.PLAYER_JS_URL) ||
      (window.ytcfg && window.ytcfg.get && window.ytcfg.get("PLAYER_JS_URL"));
    if (!url) {
      var found = /"jsUrl"\s*:\s*"([^"]+base\.js)"/.exec(
        document.documentElement.innerHTML,
      );
      url = found && found[1];
    }
    if (!url)
      return Promise.reject(new Error("the page names no player script"));
    return fetch(url).then(function (response) {
      return response.text();
    });
  }

  // The attestation constants, read out of the player script.
  //
  // Nothing here has a hardcoded fallback on purpose. A pattern only misses when Google has moved
  // the thing it was looking for, which is exactly when a remembered value is stale, so guessing
  // would trade a truthful failure for a request that is refused for a reason the log then lies
  // about. The mint is not on the playback path and a cold start token covers the gap, so failing
  // and saying which piece went missing costs one retry and buys a name to go and fix.
  function constants() {
    return player().then(function (source) {
      // The player reads its key off an experiment and keeps two literals behind it, of which the
      // web client takes the second. A third branch, or a swapped order, would need reading again.
      var branches = /html5_web_po_request_key[\s\S]*?\}/.exec(source);
      var literals = branches ? branches[0].match(/"[A-Za-z0-9_-]{20}"/g) : null;
      var key = /"X-Goog-Api-Key"\]\s*:\s*"(AIzaSy[A-Za-z0-9_-]{33})"/.exec(source);
      var host = /"(https:\/\/[a-z0-9-]+-pa\.googleapis\.com)"/.exec(source);
      var path = /"(\/google\.internal\.waa\.v1\.Waa\/GenerateIT)"/.exec(source);
      var missing = [];
      if (!literals) missing.push("request key");
      if (!key) missing.push("api key");
      if (!host) missing.push("anti-abuse host");
      if (!path) missing.push("rpc path");
      if (missing.length) {
        throw new Error("the player names no " + missing.join(" and no "));
      }
      return {
        request: literals[literals.length - 1].slice(1, -1),
        key: key[1],
        host: host[1],
        path: path[1],
      };
    });
  }

  // Trades an attestation for an integrity token. Google answers a browser it does not trust with
  // a fallback token and no integrity token at all.
  function integrity(attestation, waa) {
    return fetch(waa.host + "/$rpc" + waa.path, {
      method: "POST",
      headers: {
        "content-type": "application/json+protobuf",
        "x-goog-api-key": waa.key,
        "x-user-agent": "grpc-web-javascript/0.1",
      },
      body: JSON.stringify([waa.request, attestation]),
    })
      .then(function (response) {
        return response.json();
      })
      .then(function (issued) {
        if (!issued || !issued[0])
          throw new Error("google issued no integrity token");
        return base64ToBytes(issued[0]);
      });
  }

  function mint() {
    if (!wanted) return fail("no binding in the url");
    if (minted()) return;
    var signals = null;
    challenge()
      .then(function (bg) {
        var url =
          bg.interpreterUrl
            .privateDoNotAccessOrElseTrustedResourceUrlWrappedValue;
        return load("https:" + url).then(function () {
          return attest(bg);
        });
      })
      .then(function (attested) {
        if (!attested.signals[0])
          throw new Error("botguard did not trust this browser");
        signals = attested.signals;
        return constants().then(function (waa) {
          return integrity(attested.token, waa);
        });
      })
      .then(function (token) {
        return signals[0](token);
      })
      .then(function (minter) {
        if (typeof minter !== "function")
          throw new Error("the integrity token yielded no minter");
        return minter(new TextEncoder().encode(wanted));
      })
      .then(function (token) {
        if (!token || !token.length)
          throw new Error("the minter returned nothing");
        answer(bytesToBase64(token));
      })
      .catch(function (error) {
        fail(error && error.message ? error.message : error);
      });
  }

  // Nothing waits for the load event. A youtube page in a window no one is looking at can keep
  // loading for a long time, and the challenge arrives well before it settles; `challenge` waits
  // for that instead.
  try {
    mint();
  } catch (error) {
    fail(error && error.message ? error.message : error);
  }
})();
