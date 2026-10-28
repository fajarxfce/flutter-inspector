import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:geolocator/geolocator.dart';

void main() => runApp(const MaterialApp(home: GpsFixture()));

class GpsFixture extends StatefulWidget {
  const GpsFixture({super.key});
  @override
  State<GpsFixture> createState() => _GpsFixtureState();
}

class _GpsFixtureState extends State<GpsFixture> {
  StreamSubscription<Position>? stream;
  String status = 'Waiting for location permission';
  @override
  void initState() {
    super.initState();
    unawaited(start());
  }

  Future<void> start() async {
    var permission = await Geolocator.checkPermission();
    if (permission == LocationPermission.denied) {
      permission = await Geolocator.requestPermission();
    }
    if (!mounted) return;
    if (permission == LocationPermission.denied ||
        permission == LocationPermission.deniedForever) {
      setState(() => status = 'Location permission denied');
      return;
    }
    stream =
        Geolocator.getPositionStream(
          locationSettings: AndroidSettings(
            accuracy: LocationAccuracy.bestForNavigation,
            distanceFilter: 0,
            intervalDuration: const Duration(milliseconds: 500),
          ),
        ).listen(
          (position) {
            final value = jsonEncode({
              'latitude': position.latitude,
              'longitude': position.longitude,
              'speed': position.speed,
              'heading': position.heading,
              'altitude': position.altitude,
              'accuracy': position.accuracy,
              'mock': position.isMocked,
            });
            debugPrint('INSPECTOR_GPS_FIX $value');
            if (mounted) setState(() => status = value);
          },
          onError: (Object error) {
            debugPrint('INSPECTOR_GPS_ERROR $error');
            if (mounted) setState(() => status = error.toString());
          },
        );
  }

  @override
  void dispose() {
    unawaited(stream?.cancel());
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(title: const Text('GPS fixture · Geolocator')),
    body: Padding(
      padding: const EdgeInsets.all(24),
      child: SelectableText(status),
    ),
  );
}
