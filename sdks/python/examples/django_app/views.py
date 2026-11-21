from django.http import JsonResponse

from . import flags


def checkout(request):
    user_id = str(request.user.pk) if request.user.is_authenticated else request.session.session_key or "anon"
    enabled = flags.client.is_enabled("new-checkout", user_id, {"plan": request.GET.get("plan", "free")})
    return JsonResponse({"checkout": "v2" if enabled else "v1"})


def flag_health(request):
    return JsonResponse(flags.client.get_health())
