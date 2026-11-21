#define main notch_application_main
#include "../scripts/notch.c"
#undef main

static NotchApp test_app;
#define test_notch (*(Notch *)g_ptr_array_index(test_app.views, 0))
static const char *test_executable;

static void test_timer_pause(void) {
  FocusTimer timer = {.remaining = 1500};
  timer_toggle(&timer, 100 * G_USEC_PER_SEC);
  timer_update(&timer, 160 * G_USEC_PER_SEC + 200000);
  g_assert_cmpuint(timer.remaining, ==, 1440);
  timer_toggle(&timer, 170 * G_USEC_PER_SEC);
  g_assert_cmpuint(timer.remaining, ==, 1430);
  g_assert_cmpint(timer.deadline, ==, 0);
  timer_update(&timer, 900 * G_USEC_PER_SEC);
  g_assert_cmpuint(timer.remaining, ==, 1430);
  timer_toggle(&timer, 1000 * G_USEC_PER_SEC);
  g_assert_cmpint(timer.deadline, ==, (gint64)2430 * G_USEC_PER_SEC);
}

static void test_timer_skip(void) {
  FocusTimer timer = {.remaining = 1200};
  timer_skip(&timer, 0);
  g_assert_true(timer.on_break);
  g_assert_cmpuint(timer.remaining, ==, 300);
  g_assert_cmpint(timer.deadline, ==, 0);
  timer_toggle(&timer, 0);
  timer_skip(&timer, 10 * G_USEC_PER_SEC);
  g_assert_false(timer.on_break);
  g_assert_cmpint(timer.deadline, ==, (gint64)1510 * G_USEC_PER_SEC);
}

static void test_timer_completion(void) {
  FocusTimer timer = {.remaining = 1500};
  timer_toggle(&timer, 0);
  g_assert_null(timer_update(&timer, (gint64)1500 * G_USEC_PER_SEC - 1));
  g_assert_cmpuint(timer.remaining, ==, 1);
  g_assert_cmpstr(timer_update(&timer, (gint64)1500 * G_USEC_PER_SEC), ==,
                  "Focus");
  g_assert_cmpuint(timer.remaining, ==, 300);
  g_assert_cmpint(timer.deadline, ==, 0);
  g_assert_null(timer_update(&timer, (gint64)99999 * G_USEC_PER_SEC));
  timer_toggle(&timer, (gint64)100000 * G_USEC_PER_SEC);
  g_assert_cmpstr(timer_update(&timer, (gint64)100300 * G_USEC_PER_SEC), ==,
                  "Break");
  g_assert_cmpstr(timer_phase(&timer), ==, "Focus");
  g_assert_cmpuint(timer.remaining, ==, 1500);
}

static void test_timer_late_tick(void) {
  FocusTimer timer = {.remaining = 1500};
  timer_toggle(&timer, 0);
  g_assert_cmpstr(timer_update(&timer, (gint64)99999 * G_USEC_PER_SEC), ==,
                  "Focus");
  g_assert_null(timer_update(&timer, (gint64)100000 * G_USEC_PER_SEC));
  g_assert_cmpuint(timer.remaining, ==, 300);
}

static void test_stopwatch(void) {
  Stopwatch watch = {0};
  stopwatch_toggle(&watch, 0);
  g_assert_true(watch.running);
  g_assert_cmpint(stopwatch_elapsed(&watch, 1500000), ==, 1500000);
  stopwatch_toggle(&watch, 2000000);
  g_assert_false(watch.running);
  g_assert_cmpint(stopwatch_elapsed(&watch, 9000000), ==, 2000000);
  stopwatch_toggle(&watch, 10000000);
  g_assert_cmpint(stopwatch_elapsed(&watch, 10500000), ==, 2500000);
  g_autofree char *short_text =
      stopwatch_text((gint64)61 * G_USEC_PER_SEC + 250000);
  g_assert_cmpstr(short_text, ==, "01:01.2");
  g_autofree char *long_text = stopwatch_text((gint64)3725 * G_USEC_PER_SEC);
  g_assert_cmpstr(long_text, ==, "1:02:05");
}

static void test_weather_icon(void) {
  g_assert_cmpstr(weather_icon("Partly cloudy"), ==,
                  "weather-few-clouds-symbolic");
  g_assert_cmpstr(weather_icon("Light snow showers"), ==,
                  "weather-snow-symbolic");
  g_assert_cmpstr(weather_icon("Thundery outbreaks possible"), ==,
                  "weather-storm-symbolic");
  g_assert_cmpstr(weather_icon("Smoky haze"), ==, "weather-fog-symbolic");
  g_assert_cmpstr(weather_icon("Sunny"), ==, "weather-clear-symbolic");
  g_assert_cmpstr(weather_icon("Moderate rain"), ==,
                  "weather-showers-symbolic");
}

static void test_ease(void) {
  g_assert_cmpfloat(ease(0), ==, 0);
  g_assert_cmpfloat(ease(1), ==, 1);
  double last = 0;
  for (int i = 1; i < 100; i++) {
    double p = ease(i / 100.0);
    g_assert_cmpfloat(p, >=, last);
    g_assert_cmpfloat(p, <, 1);
    last = p;
  }
  // Bezier midpoint u = 0.5 lands at x = 0.245, y = 0.77.
  g_assert_cmpfloat_with_epsilon(ease(0.245), 0.77, 0.001);
}

static void test_level_parse(void) {
  g_assert_cmpint(parse_volume("Volume: 0.22"), ==, 22);
  g_assert_cmpint(parse_volume("Volume: 0.40 [MUTED]"), ==, 40);
  g_assert_cmpint(parse_volume("Volume: 1.50"), ==, 100);
  g_assert_cmpint(parse_volume("garbage"), ==, -1);
  g_assert_cmpint(parse_volume(NULL), ==, -1);
  g_assert_cmpint(parse_brightness("intel_backlight,backlight,208,52%,400"), ==,
                  52);
  g_assert_cmpint(parse_brightness("intel_backlight,backlight,208"), ==, -1);
  g_assert_cmpint(parse_brightness("a,b,c,x%,d"), ==, -1);
}

static void settle(void) {
  gint64 deadline = g_get_monotonic_time() + 250000;
  while (g_get_monotonic_time() < deadline) {
    while (g_main_context_iteration(NULL, FALSE)) {
    }
    g_usleep(5000);
  }
  // Frame callbacks stall while another surface covers the test window.
  if (test_app.views && test_app.views->len)
    gtk_container_check_resize(GTK_CONTAINER(test_notch.window));
}

static int window_height(Notch *n) {
  int width, height;
  gtk_window_get_size(GTK_WINDOW(n->window), &width, &height);
  g_assert_cmpint(width, ==, NOTCH_WIDTH);
  return height;
}

static void test_ui_drawer(void) {
  Notch *n = &test_notch;
  collapse_clicked(NULL, n);
  settle();
  g_assert_cmpint(window_height(n), ==, NOTCH_HEIGHT);
  GtkAllocation box;
  gtk_widget_get_allocation(n->morph, &box);
  g_assert_cmpint(box.width, ==, PILL_WIDTH);
  g_assert_cmpint(box.height, ==, PILL_HEIGHT);
  GdkEventCrossing enter = {.type = GDK_ENTER_NOTIFY,
                            .detail = GDK_NOTIFY_NONLINEAR};
  crossing(NULL, &enter, n);
  settle();
  g_assert_cmpint(n->state, ==, NOTCH_PEEK);
  gtk_widget_get_allocation(n->morph, &box);
  g_assert_cmpint(box.width, ==, NOTCH_WIDTH);
  g_assert_cmpint(box.height, ==, header_height(n));
  g_assert_false(gtk_widget_get_child_visible(n->body));
  motion(NULL, NULL, n);
  g_assert_cmpuint(n->open_source, >, 0);
  settle();
  g_assert_cmpint(n->state, ==, NOTCH_OPEN);
  g_assert_true(gtk_widget_get_child_visible(n->body));
  gtk_widget_get_allocation(n->morph, &box);
  g_assert_cmpint(box.height, >, header_height(n) * 3);
  if (g_getenv("NIRI_SOCKET"))
    g_assert_nonnull(n->blur);
  cairo_region_t *blur = cairo_region_create();
  add_rounded(blur, n->morph, n->morph, MORPH_RADIUS, n->window);
  int x, y;
  g_assert_true(
      gtk_widget_translate_coordinates(n->morph, n->window, 0, 0, &x, &y));
  g_assert_false(cairo_region_contains_point(blur, x, y));
  g_assert_true(cairo_region_contains_point(blur, x + MORPH_RADIUS, y + 1));
  g_assert_true(cairo_region_contains_point(blur, x + 1, y + 30));
  cairo_region_destroy(blur);
  g_assert_cmpint(gtk_layer_get_exclusive_zone(GTK_WINDOW(n->window)), ==, -1);
  g_assert_cmpint(gtk_layer_get_keyboard_mode(GTK_WINDOW(n->window)), ==,
                  GTK_LAYER_SHELL_KEYBOARD_MODE_ON_DEMAND);
  g_assert_cmpint(window_height(n), ==, NOTCH_HEIGHT);
  GdkEventKey escape = {.keyval = GDK_KEY_Escape};
  g_assert_true(key_pressed(NULL, &escape, n));
  settle();
  g_assert_cmpint(n->state, ==, NOTCH_HIDDEN);
  crossing(NULL, &enter, n);
  GdkEventCrossing leave = {.type = GDK_LEAVE_NOTIFY,
                            .detail = GDK_NOTIFY_NONLINEAR};
  crossing(NULL, &leave, n);
  g_assert_cmpuint(n->open_source, ==, 0);
  g_assert_cmpuint(n->close_source, >, 0);
  settle();
  settle();
  g_assert_cmpint(n->state, ==, NOTCH_HIDDEN);
  pin_clicked(NULL, n);
  g_assert_cmpint(n->state, ==, NOTCH_OPEN);
  crossing(NULL, &leave, n);
  g_assert_cmpuint(n->close_source, ==, 0);
  focus_out(NULL, NULL, n);
  g_assert_false(n->pinned);
}

static void test_ui_timer(void) {
  Notch *n = &test_notch;
  timer_clicked(NULL, n);
  g_assert_true(gtk_widget_get_visible(n->compact_timer));
  g_assert_cmpuint(n->timer_source, >, 0);
  timer_clicked(NULL, n);
  g_assert_cmpint(n->shared->timer.deadline, ==, 0);
  g_assert_cmpuint(n->timer_source, ==, 0);
  n->shared->timer.on_break = TRUE;
  n->shared->timer.remaining = 300;
  refresh_timer(n);
  g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(n->countdown)), ==, "05:00");
  reset_clicked(NULL, n);
  g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(n->countdown)), ==, "25:00");
  g_assert_false(gtk_widget_get_visible(n->compact_timer));
}

static void test_ui_stopwatch(void) {
  Notch *n = &test_notch;
  watch_clicked(NULL, n);
  g_assert_cmpuint(n->watch_source, >, 0);
  settle();
  watch_clicked(NULL, n);
  g_assert_cmpuint(n->watch_source, ==, 0);
  g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(n->watch_state)), ==, "Paused");
  g_assert_true(gtk_widget_get_visible(n->compact_timer));
  watch_reset_clicked(NULL, n);
  g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(n->watch)), ==, "00:00.0");
  g_assert_false(gtk_widget_get_visible(n->compact_timer));
}

static void test_ui_weather(void) {
  Notch *n = &test_notch;
  for (guint i = 0;
       i < 20 && !strcmp(gtk_label_get_text(GTK_LABEL(n->weather_temp)), "--");
       i++)
    settle();
  g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(n->weather_temp)), ==, "21°C");
  g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(n->forecast_place)), ==,
                  "Testville");
  g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(n->forecast_detail)), ==,
                  "Feels 20°C · 40% humidity");
  apply_weather(n, "Smoky haze|+26°C|+28°C|77%|Chennai, Tamil Nadu, IN");
  g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(n->forecast_place)), ==,
                  "Chennai");
  g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(n->forecast_detail)), ==,
                  "Feels 28°C · 77% humidity");
  apply_weather(n, "Unknown location");
  g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(n->forecast_condition)), ==,
                  "Weather unavailable");
  g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(n->weather_temp)), ==, "--");
}

static void test_ui_levels(void) {
  Notch *n = &test_notch;
  pin_clicked(NULL, n);
  for (guint i = 0;
       i < 20 && gtk_range_get_value(GTK_RANGE(n->brightness.scale)) != 52; i++)
    settle();
  g_assert_true(gtk_widget_get_visible(n->volume.row));
  g_assert_cmpfloat(gtk_range_get_value(GTK_RANGE(n->volume.scale)), ==, 40);
  g_assert_cmpfloat(gtk_range_get_value(GTK_RANGE(n->brightness.scale)), ==,
                    52);
  g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(n->brightness.value)), ==,
                  "52%");
  for (int value = 60; value <= 70; value++)
    gtk_range_set_value(GTK_RANGE(n->volume.scale), value);
  gtk_range_set_value(GTK_RANGE(n->brightness.scale), 0);
  for (guint i = 0; i < 20 && (n->volume.busy || n->brightness.busy); i++)
    settle();
  g_autofree char *log = NULL;
  g_assert_true(
      g_file_get_contents(g_getenv("NOTCH_LEVEL_LOG"), &log, NULL, NULL));
  g_assert_true(g_str_has_prefix(
      log, "wpctl set-volume -l 1 @DEFAULT_AUDIO_SINK@ 60%\n"));
  g_assert_nonnull(strstr(log, "@DEFAULT_AUDIO_SINK@ 70%\n"));
  g_assert_nonnull(strstr(log, "brightnessctl -q set 1%\n"));
  g_assert_null(strstr(log, "65%"));
  collapse_clicked(NULL, n);
}

static void test_ui_search(void) {
  Notch *n = &test_notch;
  pin_clicked(NULL, n);
  gtk_entry_set_text(GTK_ENTRY(n->search), "a");
  settle();
  g_assert_cmpint(window_height(n), ==, NOTCH_HEIGHT);
  gtk_entry_set_text(GTK_ENTRY(n->search), "zzqx-no-such-app");
  g_assert_true(gtk_widget_get_visible(n->results));
  GtkListBoxRow *row =
      gtk_list_box_get_row_at_index(GTK_LIST_BOX(n->results), 0);
  g_assert_nonnull(row);
  g_assert_null(g_object_get_data(G_OBJECT(row), "app"));
  g_assert_true(gtk_list_box_get_selected_row(GTK_LIST_BOX(n->results)) == row);
  g_assert_null(gtk_list_box_get_row_at_index(GTK_LIST_BOX(n->results), 1));
  GdkEventKey up = {.keyval = GDK_KEY_Up};
  g_assert_true(search_key(NULL, &up, n));
  g_assert_true(gtk_list_box_get_selected_row(GTK_LIST_BOX(n->results)) == row);
  gtk_entry_set_text(GTK_ENTRY(n->search), "https://example.org");
  row = gtk_list_box_get_row_at_index(GTK_LIST_BOX(n->results), 0);
  GList *cells = gtk_container_get_children(
      GTK_CONTAINER(gtk_bin_get_child(GTK_BIN(row))));
  g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(g_list_last(cells)->data)), ==,
                  "Open https://example.org");
  g_list_free(cells);
  GdkEventKey escape = {.keyval = GDK_KEY_Escape};
  g_assert_true(key_pressed(NULL, &escape, n));
  g_assert_cmpstr(gtk_entry_get_text(GTK_ENTRY(n->search)), ==, "");
  g_assert_cmpint(n->state, ==, NOTCH_OPEN);
  collapse_clicked(NULL, n);
  g_assert_cmpstr(gtk_entry_get_text(GTK_ENTRY(n->search)), ==, "");
  g_assert_false(gtk_widget_get_visible(n->results));
}

static const char *fake_xml =
    "<node><interface name='org.mpris.MediaPlayer2'>"
    "<property name='Identity' type='s' access='read'/>"
    "<property name='CanRaise' type='b' access='read'/>"
    "<property name='CanQuit' type='b' access='read'/>"
    "</interface><interface name='org.mpris.MediaPlayer2.Player'>"
    "<method name='PlayPause'/><method name='Next'/><method name='Previous'/>"
    "<property name='PlaybackStatus' type='s' access='read'/>"
    "<property name='Metadata' type='a{sv}' access='read'/>"
    "<property name='CanControl' type='b' access='read'/>"
    "<property name='CanPlay' type='b' access='read'/>"
    "<property name='CanPause' type='b' access='read'/>"
    "<property name='CanSeek' type='b' access='read'/>"
    "<property name='CanGoNext' type='b' access='read'/>"
    "<property name='CanGoPrevious' type='b' access='read'/>"
    "</interface></node>";

static gboolean fake_playing = TRUE, fake_next = FALSE;

static GVariant *
fake_property(GDBusConnection *connection G_GNUC_UNUSED,
              const char *sender G_GNUC_UNUSED, const char *path G_GNUC_UNUSED,
              const char *interface G_GNUC_UNUSED, const char *property,
              GError **error G_GNUC_UNUSED, gpointer data G_GNUC_UNUSED) {
  if (!strcmp(property, "Identity"))
    return g_variant_new_string("Notch test player");
  if (!strcmp(property, "PlaybackStatus"))
    return g_variant_new_string(fake_playing ? "Playing" : "Paused");
  if (!strcmp(property, "Metadata")) {
    GVariantBuilder metadata;
    const char *artists[] = {"Test artist", NULL};
    g_variant_builder_init(&metadata, G_VARIANT_TYPE("a{sv}"));
    g_variant_builder_add(&metadata, "{sv}", "xesam:title",
                          g_variant_new_string(fake_next
                                                   ? "Next test track"
                                                   : "<b>Test & track</b>"));
    g_variant_builder_add(&metadata, "{sv}", "xesam:artist",
                          g_variant_new_strv(artists, -1));
    return g_variant_builder_end(&metadata);
  }
  return g_variant_new_boolean(strcmp(property, "CanGoPrevious") &&
                               strcmp(property, "CanSeek"));
}

static void fake_method(GDBusConnection *connection,
                        const char *sender G_GNUC_UNUSED, const char *path,
                        const char *interface, const char *method,
                        GVariant *parameters G_GNUC_UNUSED,
                        GDBusMethodInvocation *invocation,
                        gpointer data G_GNUC_UNUSED) {
  if (!strcmp(method, "PlayPause"))
    fake_playing = !fake_playing;
  if (!strcmp(method, "Next"))
    fake_next = TRUE;
  GVariantBuilder changed, invalidated;
  g_variant_builder_init(&changed, G_VARIANT_TYPE("a{sv}"));
  g_variant_builder_add(
      &changed, "{sv}", "PlaybackStatus",
      g_variant_new_string(fake_playing ? "Playing" : "Paused"));
  g_variant_builder_add(
      &changed, "{sv}", "Metadata",
      fake_property(NULL, NULL, NULL, NULL, "Metadata", NULL, NULL));
  g_variant_builder_init(&invalidated, G_VARIANT_TYPE("as"));
  g_dbus_connection_emit_signal(
      connection, NULL, path, "org.freedesktop.DBus.Properties",
      "PropertiesChanged",
      g_variant_new("(sa{sv}as)", interface, &changed, &invalidated), NULL);
  g_dbus_method_invocation_return_value(invocation, NULL);
}

static void fake_bus_acquired(GDBusConnection *connection,
                              const char *name G_GNUC_UNUSED, gpointer data) {
  GDBusNodeInfo *info = data;
  static const GDBusInterfaceVTable vtable = {.method_call = fake_method,
                                              .get_property = fake_property};
  for (guint i = 0; info->interfaces[i]; i++)
    g_assert_cmpuint(g_dbus_connection_register_object(
                         connection, "/org/mpris/MediaPlayer2",
                         info->interfaces[i], &vtable, NULL, NULL, NULL),
                     >, 0);
}

static gboolean fake_timeout(gpointer data) {
  g_main_loop_quit(data);
  return G_SOURCE_REMOVE;
}

static int run_fake_player(void) {
  g_autoptr(GDBusNodeInfo) info = g_dbus_node_info_new_for_xml(fake_xml, NULL);
  g_autoptr(GMainLoop) loop = g_main_loop_new(NULL, FALSE);
  guint owner = g_bus_own_name(
      G_BUS_TYPE_SESSION, "org.mpris.MediaPlayer2.notchtest",
      G_BUS_NAME_OWNER_FLAGS_NONE, fake_bus_acquired, NULL, NULL, info, NULL);
  // A failed test must not leave an indefinite synthetic player behind.
  g_timeout_add_seconds(30, fake_timeout, loop);
  g_main_loop_run(loop);
  g_bus_unown_name(owner);
  return 0;
}

static void test_ui_media(void) {
  Notch *n = &test_notch;
  g_autoptr(GSubprocess) child =
      g_subprocess_new(G_SUBPROCESS_FLAGS_STDOUT_SILENCE, NULL, test_executable,
                       "--fake-player", NULL);
  g_assert_nonnull(child);
  for (guint i = 0; i < 20 && !n->player; i++)
    settle();
  g_assert_nonnull(n->player);
  g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(n->title)), ==,
                  "<b>Test & track</b>");
  g_assert_false(gtk_label_get_use_markup(GTK_LABEL(n->title)));
  g_assert_false(gtk_widget_get_sensitive(n->previous));
  g_assert_true(gtk_widget_get_sensitive(n->play));
  g_assert_true(gtk_widget_get_visible(n->header_wave));
  g_assert_true(gtk_widget_get_visible(n->card_wave));
  g_assert_true(gtk_widget_get_visible(n->media_card));
  g_assert_false(gtk_widget_get_visible(n->media_icon));
  g_assert_null(gtk_widget_get_tooltip_text(n->summary));
  g_assert_null(gtk_widget_get_tooltip_text(n->header));
  media_clicked(GTK_BUTTON(n->play), n);
  settle();
  g_autofree char *tooltip = gtk_widget_get_tooltip_text(n->play);
  g_assert_cmpstr(tooltip, ==, "Play");
  g_assert_false(gtk_widget_get_visible(n->header_wave));
  g_assert_true(gtk_widget_get_visible(n->media_icon));
  media_clicked(GTK_BUTTON(n->next), n);
  settle();
  g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(n->title)), ==,
                  "Next test track");
  g_subprocess_force_exit(child);
  g_assert_true(g_subprocess_wait(child, NULL, NULL));
  for (guint i = 0; i < 20 && n->player; i++)
    settle();
  g_assert_null(n->player);
  g_assert_false(gtk_widget_get_sensitive(n->play));
  g_assert_false(gtk_widget_get_visible(n->media_card));
  g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(n->summary)), ==,
                  "No media playing");
}

static void test_style(void) {
  g_autoptr(GError) error = NULL;
  g_assert_true(load_style(test_app.scripts, &error));
  g_assert_no_error(error);
}

static void test_ui_dictation(void) {
  Notch *n = &test_notch;
  g_autoptr(GVariant) recording = g_variant_ref_sink(
      g_variant_new("(ssddb)", "recording", "Listening", 0.7, 3.0, TRUE));
  flow_update(n, recording);
  settle();
  g_assert_true(n->flow_recording);
  g_assert_true(gtk_revealer_get_reveal_child(GTK_REVEALER(n->flow)));
  GtkStyleContext *style = gtk_widget_get_style_context(n->flow_button);
  GtkBorder margin;
  gtk_style_context_get_margin(style, gtk_style_context_get_state(style),
                               &margin);
  g_assert_cmpint(gtk_widget_get_allocated_width(n->flow_button), ==,
                  NOTCH_WIDTH);
  g_assert_cmpint(gtk_widget_get_allocated_height(n->flow_button), ==,
                  header_height(n));
  // CSS margin lands inside the allocation, which the blur region covers.
  g_assert_cmpint(margin.top + margin.bottom + margin.left + margin.right, ==,
                  0);
  if (n->state == NOTCH_HIDDEN)
    g_assert_cmpfloat(((NotchMorph *)n->morph)->height.to, ==, 0);
  g_assert_false(gtk_widget_get_visible(n->flow_label));
  g_assert_cmpfloat(n->flow_levels[G_N_ELEMENTS(n->flow_levels) - 1], ==, 0.7);
  g_assert_cmpint(gtk_layer_get_keyboard_mode(GTK_WINDOW(n->window)), ==,
                  GTK_LAYER_SHELL_KEYBOARD_MODE_NONE);
  const char *capture = g_getenv("HYPE_FLOW_CAPTURE");
  if (capture) {
    cairo_surface_t *surface = cairo_image_surface_create(
        CAIRO_FORMAT_ARGB32, gtk_widget_get_allocated_width(n->flow_button),
        gtk_widget_get_allocated_height(n->flow_button));
    cairo_t *cr = cairo_create(surface);
    gtk_widget_draw(n->flow_button, cr);
    g_assert_cmpint(cairo_surface_write_to_png(surface, capture), ==,
                    CAIRO_STATUS_SUCCESS);
    cairo_destroy(cr);
    cairo_surface_destroy(surface);
  }
  g_autoptr(GVariant) processing = g_variant_ref_sink(
      g_variant_new("(ssddb)", "transcribing", "Transcribing", 0.0, 3.0, TRUE));
  flow_update(n, processing);
  g_assert_false(n->flow_recording);
  g_assert_false(gtk_widget_get_sensitive(n->flow_toggle));
  g_assert_true(gtk_widget_get_sensitive(n->flow_cancel));
  g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(n->flow_label)), ==,
                  "Transcribing");
  g_autoptr(GVariant) malformed =
      g_variant_ref_sink(g_variant_new_string("bad"));
  flow_update(n, malformed);
  g_assert_true(n->flow_visible);
  flow_vanished(NULL, NULL, n);
  settle();
  g_assert_false(n->flow_visible);
  g_assert_false(gtk_revealer_get_reveal_child(GTK_REVEALER(n->flow)));
  g_assert_cmpint(window_height(n), ==, NOTCH_HEIGHT);
}

static void test_ui_focus(void) {
  g_assert_false(gtk_widget_get_can_focus(test_notch.header));
  g_assert_false(gtk_widget_get_focus_on_click(test_notch.volume.scale));
  g_assert_false(gtk_widget_get_focus_on_click(test_notch.brightness.scale));
  GtkWidget *focus = gtk_window_get_focus(GTK_WINDOW(test_notch.window));
  g_assert_true(focus != test_notch.header);
}

static void test_ui_views(void) {
  guint count = test_app.views->len;
  Notch *extra = view_new(&test_app, test_notch.monitor);
  g_assert_nonnull(extra);
  g_assert_cmpuint(test_app.views->len, ==, count + 1);
  timer_clicked(NULL, &test_notch);
  g_assert_cmpuint(extra->timer_source, >, 0);
  g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(extra->phase)), ==,
                  gtk_label_get_text(GTK_LABEL(test_notch.phase)));
  reset_clicked(NULL, extra);
  g_assert_cmpuint(test_notch.timer_source, ==, 0);
  g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(test_notch.countdown)), ==,
                  "25:00");
  gtk_widget_destroy(extra->window);
  g_assert_cmpuint(test_app.views->len, ==, count);
}

static void count_flow_activation(GSimpleAction *action G_GNUC_UNUSED,
                                  GVariant *parameter G_GNUC_UNUSED,
                                  gpointer data) {
  (*(guint *)data)++;
}

static void test_ui_dictation_bus(void) {
  Notch *n = &test_notch;
  g_autoptr(GDBusConnection) connection =
      g_bus_get_sync(G_BUS_TYPE_SESSION, NULL, NULL);
  g_assert_nonnull(connection);
  g_autoptr(GSimpleActionGroup) actions = g_simple_action_group_new();
  g_autoptr(GSimpleAction) status = g_simple_action_new_stateful(
      "status", NULL,
      g_variant_new("(ssddb)", "idle", "Ready", 0.0, 0.0, FALSE));
  g_autoptr(GSimpleAction) toggle = g_simple_action_new("toggle", NULL);
  guint activations = 0;
  g_signal_connect(toggle, "activate", G_CALLBACK(count_flow_activation),
                   &activations);
  g_action_map_add_action(G_ACTION_MAP(actions), G_ACTION(status));
  g_action_map_add_action(G_ACTION_MAP(actions), G_ACTION(toggle));
  guint exported = g_dbus_connection_export_action_group(
      connection, "/dev/hype/Flow", G_ACTION_GROUP(actions), NULL);
  g_assert_cmpuint(exported, >, 0);
  flow_appeared(connection, g_dbus_connection_get_unique_name(connection), "",
                n);
  settle();
  g_simple_action_set_state(status, g_variant_new("(ssddb)", "recording",
                                                  "Listening", 0.4, 2.0, TRUE));
  settle();
  g_assert_true(n->flow_visible);
  g_assert_true(n->flow_recording);
  flow_activate(n, "toggle");
  settle();
  g_assert_cmpuint(activations, ==, 1);
  g_dbus_connection_unexport_action_group(connection, exported);
  flow_vanished(NULL, NULL, n);
  g_assert_false(n->flow_visible);
}

int main(int argc, char **argv) {
  if (argc == 2 && !strcmp(argv[1], "--fake-player"))
    return run_fake_player();
  g_assert_cmpint(argc, ==, 2);
  test_executable = argv[0];
  test_app.scripts = argv[1];
  test_app.timer.remaining = 1500;
  argc = 1;
  g_test_init(&argc, &argv, NULL);
  g_test_add_func("/notch/timer/pause-resume", test_timer_pause);
  g_test_add_func("/notch/timer/completion", test_timer_completion);
  g_test_add_func("/notch/timer/skip", test_timer_skip);
  g_test_add_func("/notch/timer/late-tick", test_timer_late_tick);
  g_test_add_func("/notch/stopwatch", test_stopwatch);
  g_test_add_func("/notch/weather-icon", test_weather_icon);
  g_test_add_func("/notch/level-parse", test_level_parse);
  g_test_add_func("/notch/ease", test_ease);
  g_test_add_func("/notch/style", test_style);
  if (g_strcmp0(g_getenv("HYPE_NOTCH_UI_TESTS"), "1") == 0) {
    g_assert_true(gtk_init_check(NULL, NULL));
    g_assert_true(gtk_layer_is_supported());
    g_object_set(gtk_settings_get_default(), "gtk-enable-animations", FALSE,
                 NULL);
    test_app.app = gtk_application_new("dev.hype_niri.NotchTest",
                                       G_APPLICATION_HANDLES_COMMAND_LINE);
    g_assert_true(
        g_application_register(G_APPLICATION(test_app.app), NULL, NULL));
    activate(test_app.app, &test_app);
    g_assert_cmpuint(test_app.views->len, ==,
                     gdk_display_get_n_monitors(gdk_display_get_default()));
    g_signal_handlers_disconnect_by_func(test_notch.window, crossing,
                                         &test_notch);
    g_signal_handlers_disconnect_by_func(test_notch.window, motion,
                                         &test_notch);
    g_signal_handlers_disconnect_by_func(test_notch.window, focus_out,
                                         &test_notch);
    g_test_add_func("/notch/ui/drawer", test_ui_drawer);
    g_test_add_func("/notch/ui/timer", test_ui_timer);
    g_test_add_func("/notch/ui/stopwatch", test_ui_stopwatch);
    g_test_add_func("/notch/ui/weather", test_ui_weather);
    g_test_add_func("/notch/ui/levels", test_ui_levels);
    g_test_add_func("/notch/ui/search", test_ui_search);
    g_test_add_func("/notch/ui/media", test_ui_media);
    g_test_add_func("/notch/ui/dictation", test_ui_dictation);
    g_test_add_func("/notch/ui/dictation-bus", test_ui_dictation_bus);
    g_test_add_func("/notch/ui/views", test_ui_views);
    g_test_add_func("/notch/ui/focus", test_ui_focus);
  }
  int result = g_test_run();
  cleanup(&test_app);
  return result;
}
